use client_api::ws::{ConnectState, WSClient, WSClientConfig};
use client_api_test_util::generate_unique_registered_user_client;
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
async fn wasm_websocket_connect_test() {
  let (c, _user) = generate_unique_registered_user_client().await;
  let ws_client = WSClient::new(WSClientConfig::default(), c.clone());
  let mut state = ws_client.subscribe_connect_state();
  let device_id = "fake_device_id";

  wasm_bindgen_futures::spawn_local(async move {
    ws_client
      .connect(c.ws_url(device_id).await.unwrap(), device_id)
      .await
      .unwrap();
  });

  // wait for the connect state to be connected
  while let Ok(new_state) = state.recv().await {
    if new_state == ConnectState::Connected {
      break;
    }
  }
}
