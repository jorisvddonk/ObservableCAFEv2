// comfy — image generation agent (ComfyUI workflow).
//
// JS port of agents/comfy.toml: user message -> llm (writes an image prompt),
// then on completion run the ComfyUI workflow. Gated on
// `config.comfy.enabled` (default false, the TOML `enabled_if`).
//
// The legacy step sent the prompt as `prompt`, but cafe-comfy reads `text`,
// so it always errored with "text param is empty". This sends `text` (plus
// optional workflow_path / input_node overrides from session config).

const manifest = {
  name: "comfy",
  description: "Image generation agent — runs a ComfyUI workflow and produces an image",
  background: false,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
  initial_config: {
    "config.type": "runtime",
    "config.llm.system_prompt":
      "You are an image generation assistant. Respond with a detailed image prompt describing what to generate. Keep your response to just the prompt text.",
    "config.llm.temperature": 0.8,
    "config.comfy.workflow_path": "workflow.json",
    "config.comfy.workflow_input_node": "6",
    "config.comfy.enabled": false,
  },
};

async function main(cafe) {
  const cfg = await cafe.config();
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("llm", {});
    } else if (event.type === "llm_complete" && cfg["config.comfy.enabled"] === true) {
      const params = { text: event.text };
      if (cfg["config.comfy.workflow_path"]) {
        params.workflow_path = cfg["config.comfy.workflow_path"];
      }
      if (cfg["config.comfy.workflow_input_node"]) {
        params.input_node = cfg["config.comfy.workflow_input_node"];
      }
      await cafe.invoke("comfy", params);
    }
  }
  return "comfy: done";
}
