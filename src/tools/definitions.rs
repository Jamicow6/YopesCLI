//! The JSON schemas for every tool, sent to the provider with each request.
//!
//! One entry per tool in `TOOL_NAMES`; the test in `mod.rs` checks the two
//! lists stay in sync.

use serde_json::{Value, json};

pub fn definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and directories in the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path relative to the working directory. Defaults to ."
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a UTF-8 text file, optionally a range of lines. A directory path lists its entries.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "First line to read, starting at 1."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of lines to return."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "multi_read",
                "description": "Read several UTF-8 text files in one call, in order. With all=true every entry in paths is a folder and every file directly inside it is read. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "paths": {
                            "type": "array",
                            "description": "File paths (or folder paths when all is true), relative to the working directory.",
                            "items": {
                                "type": "string"
                            }
                        },
                        "all": {
                            "type": "boolean",
                            "description": "Read every file directly inside each folder in paths instead of the paths themselves."
                        }
                    },
                    "required": ["paths"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_image",
                "description": "Attach a PNG or JPEG image from disk to the conversation so a vision-capable model can see it. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Image path relative to the working directory."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "glob",
                "description": "Find files and directories by glob pattern, relative to the working directory. Supports * (any text within one path segment), ? (one character), and ** (any number of segments). Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern such as src/**/*.rs."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search under, relative to the working directory. Defaults to ."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Search file contents with a plain substring and return file:line:text matches. Binary files, dependency and cache directories are skipped. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Text to search for."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search under, relative to the working directory. Defaults to ."
                        },
                        "include": {
                            "type": "string",
                            "description": "Glob for file types, such as *.rs or src/*/*.ts."
                        },
                        "ignore_case": {
                            "type": "boolean",
                            "description": "Match without case sensitivity. Defaults to false."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a text file. Parent directories are created.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "content": {
                            "type": "string",
                            "description": "Full file content to write."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace exact text in an existing UTF-8 file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "old_text": {
                            "type": "string",
                            "description": "Exact text to find."
                        },
                        "new_text": {
                            "type": "string",
                            "description": "Replacement text."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace every match instead of requiring exactly one."
                        }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "multi_edit",
                "description": "Apply several exact-text edits in one call. Every edit follows the edit_file rules against one in-memory state, so a batch either applies fully or refuses without touching any file. Sequential edits to the same file see each other's results.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "edits": {
                            "type": "array",
                            "description": "The edits, in order.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "File path relative to the working directory."
                                    },
                                    "old_text": {
                                        "type": "string",
                                        "description": "Exact text to find."
                                    },
                                    "new_text": {
                                        "type": "string",
                                        "description": "Replacement text."
                                    },
                                    "replace_all": {
                                        "type": "boolean",
                                        "description": "Replace every match in that file instead of requiring exactly one."
                                    }
                                },
                                "required": ["path", "old_text", "new_text"]
                            }
                        }
                    },
                    "required": ["edits"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command in the workspace and return its output. Set risk to true for anything destructive or hard to undo: deleting or overwriting files, git resets or force pushes, installing or uninstalling software, stopping services, or anything reaching outside the working directory. Risky commands ask the user before they run. When the user requests running commands or actions, execute them directly with this tool rather than just describing the command.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to run from the working directory."
                        },
                        "risk": {
                            "type": "boolean",
                            "description": "True when the command is destructive or could discard work, so the user is asked first."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum run time in seconds. Defaults to 120."
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_background",
                "description": "Start one bash shell command in the background and get a job id. Its output lands in a log: read it with bash_output, stop it with bash_kill. Set risk to true for anything destructive or hard to undo so the user is asked first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Bash command to start."
                        },
                        "risk": {
                            "type": "boolean",
                            "description": "True when the command is destructive or could discard work, so the user is asked first."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Kill the job after this many seconds. Defaults to 120."
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_output",
                "description": "Read a background bash job: everything it printed so far and whether it still runs or has exited. Pass the job id from bash_background.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": {
                            "type": "integer",
                            "description": "The id bash_background returned."
                        }
                    },
                    "required": ["job_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_kill",
                "description": "Stop a background bash job by its id. A finished or unknown job answers so, no need to kill twice.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": {
                            "type": "integer",
                            "description": "The id bash_background returned."
                        }
                    },
                    "required": ["job_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web and return the top results with titles, URLs and snippets. Use it for current information, documentation, release notes, or anything the user asks about that is newer than your knowledge. Cite the URL you used.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query."
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results. Defaults to 5."
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch one web page over HTTP GET and return its readable text: scripts, styles and tags are stripped. Use it when search results point at a page worth reading or when the user gives an address.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Absolute http:// or https:// address."
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_write",
                "description": "Maintain the session task checklist dynamically. Rewrite the whole list each call: every item is an object with the task text and a done flag. Send an empty items list to close the checklist once everything is done. The list is stored with the session, not as a file. You MUST use this tool dynamically to track any multi-step plan, updating task done states as you work.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "description": "The full checklist.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": {
                                        "type": "string",
                                        "description": "The task."
                                    },
                                    "done": {
                                        "type": "boolean",
                                        "description": "Whether the task is finished. Defaults to false."
                                    }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["items"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "delegate",
                "description": "Hand one self-contained subtask to a subagent running on the same provider and model, and receive its final answer. The subagent has no tools: it cannot read files, run commands, or search the web, so the task must include every file excerpt, code snippet, and fact it needs. Use it for focused drafts, second opinions, or research recaps, and split large work into several delegate calls.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "Complete instructions for the subagent, including all context it needs."
                        }
                    },
                    "required": ["task"]
                }
            }
        }
    ])
}
