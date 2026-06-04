pub struct Config {
    pub socket_path: String,
    pub comfy_url: String,
    pub workflow_path: String,
    pub workflow_input_node: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            comfy_url: std::env::var("COMFY_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8188".into()),
            workflow_path: std::env::var("COMFY_WORKFLOW_PATH")
                .unwrap_or_else(|_| "workflow.json".into()),
            workflow_input_node: std::env::var("COMFY_WORKFLOW_INPUT_NODE")
                .unwrap_or_else(|_| "6".into()),
        }
    }
}
