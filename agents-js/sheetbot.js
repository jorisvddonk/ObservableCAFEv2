// sheetbot — SheetBot integration: tasks, sheets and artefacts via RPC.
//
// JS port of agents/sheetbot.toml. The LLM emits `<|tool_call|>` markers
// naming `sheetbot.*` methods; the agent executes each via cafe.tool (which
// publishes the bus-visible result) and then lets the LLM answer. The
// initial_config below is generated verbatim from the TOML (system prompt +
// 19 tool definitions).

const manifest = {
  name: "sheetbot",
  description: "SheetBot integration \u2014 create and manage SheetBot tasks, sheets, and artefacts via RPC",
  background: true,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
  initial_config: {
  "config.type": "runtime",
  "config.llm.system_prompt": "You are a helpful assistant with access to the SheetBot task and sheet management system.\n\nYou can invoke tools by emitting `<|tool_call|>` markers in your response:\n\n<|tool_call|>{\"name\":\"sheetbot.list_tasks\",\"parameters\":{}}<|tool_call_end|>\n\nAvailable tools:\n- sheetbot.list_tasks — List all tasks, optionally filtered by status (0=awaiting, 1=running, 2=completed, 3=failed, 4=paused)\n- sheetbot.get_task — Get details of a specific task (requires: id)\n- sheetbot.create_task — Create a new task (requires: script, optional: name, type, data)\n- sheetbot.update_task — Update a task (requires: id)\n- sheetbot.delete_task — Delete a task (requires: id)\n- sheetbot.accept_task — Accept a task (requires: id)\n- sheetbot.complete_task — Complete a task (requires: id)\n- sheetbot.fail_task — Fail a task (requires: id)\n- sheetbot.clone_task — Clone a task (requires: id)\n- sheetbot.get_next_task — Get the next awaiting task\n- sheetbot.update_task_data — Update task data (requires: id)\n- sheetbot.list_sheets — List all available sheets\n- sheetbot.get_sheet — Get data from a specific sheet (requires: id)\n- sheetbot.upsert_sheet_data — Upsert data into a sheet (requires: id)\n- sheetbot.delete_sheet_row — Delete a row from a sheet (requires: id, key)\n- sheetbot.upload_artefact — Upload an artefact to a task (requires: task_id, filename)\n- sheetbot.get_artefact — Get an artefact from a task (requires: task_id, filename)\n- sheetbot.delete_artefact — Delete an artefact from a task (requires: task_id, filename)\n- sheetbot.list_library — List available scripts in the SheetBot library\n\nTask statuses:\n- 0 (AWAITING): Ready for execution\n- 1 (RUNNING): Currently being executed\n- 2 (COMPLETED): Finished successfully\n- 3 (FAILED): Finished with error\n- 4 (PAUSED): Not ready for execution\n",
  "config.sheetbot.url": "http://localhost:3000",
  "tools.available": [
    {
      "name": "sheetbot.list_tasks",
      "description": "List all tasks, optionally filtered by status",
      "parameters": {
        "type": "object",
        "properties": {
          "status": {
            "type": "integer",
            "description": "Filter by status (0=AWAITING, 1=RUNNING, 2=COMPLETED, 3=FAILED, 4=PAUSED)"
          }
        }
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.get_task",
      "description": "Get details of a specific task",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.create_task",
      "description": "Create a new task",
      "parameters": {
        "type": "object",
        "properties": {
          "script": {
            "type": "string",
            "description": "Task script content"
          },
          "name": {
            "type": "string",
            "description": "Task name"
          },
          "type": {
            "type": "string",
            "enum": [
              "deno",
              "python",
              "bash"
            ],
            "description": "Execution type"
          },
          "data": {
            "type": "object",
            "description": "JSON data"
          }
        },
        "required": [
          "script"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.update_task",
      "description": "Update a task",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.delete_task",
      "description": "Delete a task",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.accept_task",
      "description": "Accept a task for execution",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.complete_task",
      "description": "Complete a task",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.fail_task",
      "description": "Mark a task as failed",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.clone_task",
      "description": "Clone a task",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.get_next_task",
      "description": "Get the next awaiting task",
      "parameters": {
        "type": "object",
        "properties": {}
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.update_task_data",
      "description": "Update task data",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Task UUID"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.list_sheets",
      "description": "List all available sheets",
      "parameters": {
        "type": "object",
        "properties": {}
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.get_sheet",
      "description": "Get data from a specific sheet",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Sheet name"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.upsert_sheet_data",
      "description": "Upsert data into a sheet",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Sheet name"
          }
        },
        "required": [
          "id"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.delete_sheet_row",
      "description": "Delete a row from a sheet",
      "parameters": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Sheet name"
          },
          "key": {
            "type": "string",
            "description": "Row key"
          }
        },
        "required": [
          "id",
          "key"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.upload_artefact",
      "description": "Upload an artefact to a task",
      "parameters": {
        "type": "object",
        "properties": {
          "task_id": {
            "type": "string",
            "description": "Task UUID"
          },
          "filename": {
            "type": "string",
            "description": "Artefact filename"
          }
        },
        "required": [
          "task_id",
          "filename"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.get_artefact",
      "description": "Get an artefact from a task",
      "parameters": {
        "type": "object",
        "properties": {
          "task_id": {
            "type": "string",
            "description": "Task UUID"
          },
          "filename": {
            "type": "string",
            "description": "Artefact filename"
          }
        },
        "required": [
          "task_id",
          "filename"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.delete_artefact",
      "description": "Delete an artefact from a task",
      "parameters": {
        "type": "object",
        "properties": {
          "task_id": {
            "type": "string",
            "description": "Task UUID"
          },
          "filename": {
            "type": "string",
            "description": "Artefact filename"
          }
        },
        "required": [
          "task_id",
          "filename"
        ]
      },
      "tool_type": "rpc"
    },
    {
      "name": "sheetbot.list_library",
      "description": "List available scripts in the SheetBot library",
      "parameters": {
        "type": "object",
        "properties": {}
      },
      "tool_type": "rpc"
    }
  ]
},
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("llm", {});
    } else if (event.type === "llm_complete") {
      let called = false;
      for (const call of cafe.findToolCalls(event.text)) {
        called = true;
        await cafe.tool(call.name, call.parameters || {});
      }
      if (called) {
        await cafe.invoke("llm", {});
      }
    }
  }
  return "sheetbot: done";
}

