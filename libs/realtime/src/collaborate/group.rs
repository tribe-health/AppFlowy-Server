use crate::collaborate::group_control::CollabGroupControl;
use crate::collaborate::group_sub::{CollabUserMessage, SubscribeGroup};
use crate::collaborate::{broadcast_message, CollabAccessControl, CollabClientStream};
use crate::entities::{Editing, RealtimeUser};
use crate::error::RealtimeError;
use anyhow::anyhow;
use async_stream::stream;
use dashmap::DashMap;
use database::collab::CollabStorage;
use futures_util::StreamExt;
use realtime_entity::collab_msg::{CollabMessage, CollabSinkMessage};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{error, trace};

pub enum GroupCommand<U> {
  HandleCollabMessage {
    user: U,
    collab_message: CollabMessage,
  },
}

pub type GroupControlCommandSender<U> = tokio::sync::mpsc::Sender<GroupCommand<U>>;
pub type GroupControlCommandReceiver<U> = tokio::sync::mpsc::Receiver<GroupCommand<U>>;

pub struct GroupCommandRunner<S, U, AC> {
  pub group_control: Arc<CollabGroupControl<S, U, AC>>,
  pub client_stream_by_user: Arc<DashMap<U, CollabClientStream>>,
  pub edit_collab_by_user: Arc<DashMap<U, HashSet<Editing>>>,
  pub access_control: Arc<AC>,
  pub recv: Option<GroupControlCommandReceiver<U>>,
}

impl<S, U, AC> GroupCommandRunner<S, U, AC>
where
  S: CollabStorage,
  U: RealtimeUser,
  AC: CollabAccessControl,
{
  pub async fn run(mut self, object_id: String) {
    let mut receiver = self.recv.take().expect("Only take once");
    let stream = stream! {
      while let Some(msg) = receiver.recv().await {
         yield msg;
      }
      trace!("Collab group:{} command runner is stopped", object_id);
    };

    stream
      .for_each(|command| async {
        match command {
          GroupCommand::HandleCollabMessage {
            user,
            collab_message,
          } => {
            if let Err(err) = self.handle_collab_message(user, collab_message).await {
              error!("Failed to handle collab message: {}", err);
            }
          },
        }
      })
      .await;
  }

  /// Processes a client message with the following logic:
  /// 1. Verifies client connection to the websocket server.
  /// 2. Processes [CollabMessage] messages as follows:
  ///    2.1 For 'init sync' messages:
  ///      - If the group exists: Removes the old subscription and re-subscribes the user.
  ///      - If the group does not exist: Creates a new group.
  ///      In both cases, the message is then sent to the group for synchronization according to [CollabSyncProtocol],
  ///      which includes broadcasting to all connected clients.
  ///    2.2 For non-'init sync' messages:
  ///      - If the group exists: The message is sent to the group for synchronization as per [CollabSyncProtocol].
  ///      - If the group does not exist: The client is prompted to send an 'init sync' message first.

  async fn handle_collab_message(
    &self,
    user: U,
    collab_message: CollabMessage,
  ) -> Result<(), RealtimeError> {
    // 1.Check the client is connected with the websocket server.
    if self.client_stream_by_user.get(&user).is_none() {
      let msg = anyhow!("The client stream: {} is not found, it should be created when the client is connected with this websocket server", user);
      return Err(RealtimeError::Internal(msg));
    }

    let is_group_exist = self
      .group_control
      .contains_group(collab_message.object_id())
      .await;
    if is_group_exist {
      // If a group exists for the specified object_id and the message is an 'init sync',
      // then remove any existing subscriber from that group and add the new user as a subscriber to the group.
      if collab_message.is_init_msg() {
        self
          .group_control
          .remove_user(collab_message.object_id(), &user)
          .await?;
      }

      // subscribe the user to the group. then the user will receive the changes from the group
      let is_user_subscribed = self
        .group_control
        .contains_user(collab_message.object_id(), &user)
        .await;
      if !is_user_subscribed {
        self.subscribe_group(&user, &collab_message).await?;
      }
      broadcast_message(&user, collab_message, &self.client_stream_by_user).await;
    } else {
      // If there is no existing group for the given object_id and the message is an 'init message',
      // then create a new group and add the user as a subscriber to this group.
      if collab_message.is_init_msg() {
        self.create_group(&collab_message).await?;
        self.subscribe_group(&user, &collab_message).await?;
      } else {
        // TODO(nathan): ask the client to send the init message first
      }
    }
    Ok(())
  }

  async fn subscribe_group(
    &self,
    user: &U,
    collab_message: &CollabMessage,
  ) -> Result<(), RealtimeError> {
    SubscribeGroup {
      message: &CollabUserMessage {
        user,
        collab_message,
      },
      groups: &self.group_control,
      edit_collab_by_user: &self.edit_collab_by_user,
      client_stream_by_user: &self.client_stream_by_user,
      access_control: &self.access_control,
    }
    .run()
    .await;
    Ok(())
  }
  async fn create_group(&self, collab_message: &CollabMessage) -> Result<(), RealtimeError> {
    let object_id = collab_message.object_id();
    match collab_message {
      CollabMessage::ClientInitSync(client_init) => {
        let uid = client_init
          .origin
          .client_user_id()
          .ok_or(RealtimeError::ExpectInitSync(
            "The client user id is empty".to_string(),
          ))?;

        self
          .group_control
          .create_group(
            uid,
            &client_init.workspace_id,
            object_id,
            client_init.collab_type.clone(),
          )
          .await;

        Ok(())
      },
      _ => Err(RealtimeError::ExpectInitSync(collab_message.to_string())),
    }
  }
}
