YopesCLI Major Update list









Add bash support (default git bash, disable use if git isnt installed on user machine), Read shouldnt show tool output, edit should have the diff viewer (with the insertion and deletion colors), add multi-edit (basically batching edits), write should show the diff with the color, glob tool for folders, grep to filter file type, read . to read the directory first, web search and web fetch dedicated tools, subagents delegation, proper TODO UI (without md file)
Proper file pasting from clipboard too
and image pasting 
Please reason for longer
/effort for thinking reasoning (off, none, low, medium, high, xhigh (for supported models)
support for backgrounded commands. bash_background bash_output
for the TODO I would like this native and also not distrurpuve the user experience 
like this below me

YopesCLI Tasks

✓ Inspect project structure
✓ Read package.json
● Implement authentication
○ Add tests
○ Fix TypeScript errors

Progress: 2/5


and you can use this entire list generated to make this into an actual AI harness then call this a major update 


--------------------------------------------
Your list is already a pretty solid foundation for an AI coding harness. You're essentially building a terminal-first AI agent with IDE-like editing, Claude Code-style tools, and a proper interactive UI.

The biggest things missing aren't necessarily more tools. They're execution safety, context management, agent control, and reliability.

Here's what I'd add to YopesCLI.

# 1. Your existing feature list, refined

| Feature          | Recommendation                                                                                |
| ---------------- | --------------------------------------------------------------------------------------------- |
| Bash support     | Default to Git Bash on Windows; disable shell support gracefully if Git isn't installed.      |
| Read             | Return content to the model silently; don't print tool output into the terminal conversation. |
| Edit             | Interactive diff viewer with green insertions and red deletions.                              |
| Multi-edit       | Batch multiple edits into one operation, with atomic validation.                              |
| Write            | Show a colored diff before applying changes.                                                  |
| Glob             | File discovery by patterns, with ignore rules.                                                |
| Grep             | Search file contents, with file-type filters.                                                 |
| Read `.`         | Allow reading the current directory as a directory listing.                                   |
| Web search       | Dedicated search tool.                                                                        |
| Web fetch        | Dedicated webpage fetching and extraction tool.                                               |
| Subagents        | Delegate work to independent agents with their own contexts.                                  |
| TODO             | Native task UI, not an MD file.                                                               |
| Clipboard        | Paste files, code, and images directly into the conversation.                                 |
| Reasoning effort | `/effort off`, `none`, `low`, `medium`, `high`, `xhigh`.                                      |

One important distinction: Git Bash should be a default, not a hard dependency for the harness itself. If Git isn't installed, the AI can still operate in read-only mode, use whatever supported tools remain available, and explain that shell execution is unavailable.

# 2. Essential tools you're missing

These are the additions I'd prioritize.

## File and project tools

| Tool          | Purpose                                                                 |
| ------------- | ----------------------------------------------------------------------- |
| `ls`          | Structured directory listing with hidden files, sizes, and file types.  |
| `stat`        | File metadata: size, modification time, permissions.                    |
| `find`        | Locate files by name, extension, directory, or other criteria.          |
| `tree`        | Visualize project structure with depth limits.                          |
| `symbols`     | Extract functions, classes, methods, and definitions.                   |
| `diagnostics` | Retrieve compiler errors, lint errors, and language-server diagnostics. |
| `references`  | Find references to a symbol.                                            |
| `definition`  | Jump to a symbol's definition.                                          |

The most valuable addition here is `diagnostics`.

Imagine your AI edits a TypeScript file and immediately receives:

- 3 TypeScript errors
- 1 ESLint warning
- 2 unresolved imports

It can then fix those problems before finishing.

## Shell tools

I'd implement these separately:

- `bash` — execute commands.
- `bash_background` — run long-running processes.
- `bash_output` — retrieve background process output.
- `bash_kill` — terminate a process.
- `bash_list` — list active processes.

This makes development servers, builds, and tests much more manageable.

For example:

```
> Run npm run dev

Started background process #2

> Check its output

Process #2:
VITE ready at localhost:5173
```

Don't force the AI to keep a shell call open while waiting for a server.

# 3. Proper interactive diff system

This deserves its own subsystem.

[Better Terminal Git Diffs | Mitchell Hanberg](https://images.openai.com/static-rsc-4/9R-obeXCbSowWRKpvxzzEI6iSfQZr6YDRnHKfvAERWYFxQEXrRD1j0W-tlzE-qsSFLhX-Vf6_pWsfPA1AhTdqEgQlB2kisV-7TWZoXaxaZAyemN15kquib_kTmB8lseVOFGrOvjNotGogvY5NpZR6rq3wvvDgLglN35K0pEyZfk?purpose=inline)

[Install git-tui on Linux | Snap Store](https://images.openai.com/static-rsc-4/wue2q1EOJzW8MjNVKAinlxEM6LmA5qL3vVcgXdie6P601L56S3JISsX8ehrKmWAh5xhhS2sGtnEdeEEK3gG33Ke5Cd-6fW2irbkrH0nLzG3Xzg-V8qRqS-b9ZEmavCVw0lI5vGV-nS_LM_29qudl0RQkB6wWGcdq7kD14PnyGCE?purpose=inline)

[git - Magit: how to disable highlight in the selected diff hunk - Stack Overflow](https://images.openai.com/static-rsc-4/GElpDsG7v7nGFFSa6e98LvS5LL3cCMWWm11UgTOsBgHnjKhWSZpYLt5XUox0XlGWcqwC7wKEPGjNo4m-wSsssdpBk7VhK5TjdVMOq1nQf525D5boQJRgeXJvrwwe-GW0G9qSey6_IQzs9A9jPDTsdcFmWiJdZ0R4tdB9l-VKLBQ?purpose=inline)

10

I'd make edit and write tools support three modes:

- `auto` — automatically apply when safe.
- `review` — show the diff and ask for approval.
- `manual` — never modify files until approved.

Example:

```
╭─ File Change ──────────────────────────────╮
│ src/index.ts                              │
├────────────────────────────────────────────┤
│ - console.log("Hello");                   │
│ + console.log("Hello YopesCLI");           │
├────────────────────────────────────────────┤
│ [a] Accept  [r] Reject  [e] Edit  [d] Diff │
╰────────────────────────────────────────────╯
```

The diff viewer should support:

- Inline diff
- Side-by-side diff
- Syntax highlighting
- Context expansion
- Multiple files in one review
- Accept/reject individual hunks
- Accept/reject entire changesets
- Undo applied changes

Important: validate the original file hasn't changed since the edit was prepared. Otherwise, reject the patch or regenerate it instead of overwriting the user's changes.

# 4. Native TODO and task management

Absolutely avoid requiring a TODO.md file.

Build a native task manager backed by structured state.

[Claude Redesigns Claude Code Desktop to Support Parallel Agentic Sessions](https://images.openai.com/static-rsc-4/mqnewjUHhyc4R86f6G42mkWyC0XS9gy8rDsj4FqQOEljNNOpdvK1DXGA51KKJvoPDMt3xtaJyIOq0Ip_v7qxcOaw9NWVkGBoulXIuAhAiwQgZTJgSKX6MBCPIVSG2Bcf94jY51xiQXKVB3xu8kpcUDeJBMAJEmxJ4MM1ro2DSDo?purpose=inline)

[headsupai.io](https://headsupai.io/updates/claude-redesigns-claude-code-desktop-support-parallel-agentic-sessions?utm_source=chatgpt.com)

Example:

```
YopesCLI Tasks

✓ Inspect project structure
✓ Read package.json
● Implement authentication
○ Add tests
○ Fix TypeScript errors

Progress: 2/5
```

I'd support:

- `task_create`
- `task_update`
- `task_list`
- `task_complete`
- `task_delete`
- Task dependencies
- Task priorities
- Task ownership
- Task status
- Progress tracking

Tasks should persist across sessions, preferably in a dedicated `.yopes/` state directory.

The model shouldn't need to manually maintain Markdown checkboxes.

# 5. Subagents and delegation

This is probably one of the most important features you can add.

[The Architecture of Scale: A Deep Dive into Anthropic’s Sub-Agents | by Jiten Oswal | Feb, 2026 | Medium](https://images.openai.com/static-rsc-4/WtPMVyEvosbUXDaNp3KJHzN9WCFzGhd9kDY9-cIaPYhIgAgYEat8NVX5LrlAPGUI-CtH57nmY1Hurp60nopUQgLlrBsNEznLXUBYoPcjVqNOILb6rxuvys8Q-DKQkeocH5uoiuFZybCEcjg2-LJgSqaUmtRifYIdNQaviDX4oUI?purpose=inline)

[medium.com](https://medium.com/%40jiten.p.oswal/the-architecture-of-scale-a-deep-dive-into-anthropics-sub-agents-6c4faae1abda?utm_source=chatgpt.com)

I'd implement:

| Capability      | Description                            |
| --------------- | -------------------------------------- |
| `delegate`      | Spawn a subagent with a specific task. |
| `agent_list`    | List active and completed agents.      |
| `agent_status`  | Retrieve agent progress.               |
| `agent_result`  | Retrieve completed agent output.       |
| `agent_cancel`  | Cancel an agent.                       |
| `agent_message` | Send instructions to an agent.         |

Example:

```
Main Agent
├── Agent 1: Analyze authentication
├── Agent 2: Inspect database architecture
└── Agent 3: Find existing tests
```

### Rules I'd enforce

- Every subagent has its own context window.
- Agents receive only the files and instructions they need.
- Subagents can have different models.
- Subagents can have different reasoning effort.
- Agents report structured results.
- File modifications are coordinated.
- The main agent owns final integration.
- Subagents cannot recursively spawn unlimited agents.
- Resource limits prevent runaway delegation.

I'd start with read-only subagents first, then introduce parallel editing with conflict detection.

# 6. Clipboard and image support

This is another area where a terminal harness can feel significantly better than a traditional CLI.

### Clipboard

Support:

- Ctrl+V for text.
- Ctrl+V for images.
- Ctrl+V for file paths.
- Drag-and-drop files into the terminal.
- Paste large code blocks without corrupting input.
- Paste multiple files at once.

### Image pasting

When an image is pasted:

```
> What is wrong with this UI?

[Image attached: screenshot.png]
```

The image should be passed directly to vision-capable models.

I'd also support:

- PNG
- JPEG
- WebP
- Clipboard screenshots
- Multiple image attachments
- Image previews
- Image removal before sending

On Windows, clipboard image handling should use native clipboard APIs or a reliable library rather than relying on Git Bash's clipboard behavior.

# 7. Reasoning effort and model controls

Your `/effort` idea is excellent.

I'd implement:

```
/effort off
/effort none
/effort low
/effort medium
/effort high
/effort xhigh
```

However, I'd distinguish these carefully:

| Setting  | Meaning                                                    |
| -------- | ---------------------------------------------------------- |
| `off`    | Disable configurable reasoning effort; use model defaults. |
| `none`   | Request minimal or zero reasoning where supported.         |
| `low`    | Lightweight reasoning.                                     |
| `medium` | Balanced reasoning.                                        |
| `high`   | More extensive reasoning.                                  |
| `xhigh`  | Maximum supported effort.                                  |

Unsupported values should gracefully fall back to the model's default.

Also add:

```
/model
/models
/effort
/temperature
/context
/usage
```

`/context` should show context utilization, while `/usage` should show token usage and costs where available.

# 8. Context management (VERY important)

This is the feature I'd put alongside subagents in importance.

Your harness needs an intelligent context system.

I'd implement:

- Automatic conversation summarization.
- Context compaction.
- File-based context caching.
- Reuse of previously read files.
- Automatic removal of irrelevant tool outputs.
- Token budgeting.
- Context utilization indicators.
- Project-specific instructions.
- Persistent session memory.

Example:

```
Context: 72%

System instructions     4,200 tokens
Conversation          18,500 tokens
Files                 9,800 tokens
Tool results          3,200 tokens
Available             7,300 tokens
```

When approaching the context limit:

```
Compacting conversation...

✓ Preserved task objectives
✓ Preserved implementation decisions
✓ Preserved modified files
✓ Preserved unresolved issues

Context reduced: 84,000 → 19,000 tokens
```

The model should never need to remember every single tool call from the beginning of the session.

# 9. Other features I'd definitely add

| Feature        | Why it matters                         |
| -------------- | -------------------------------------- |
| `/undo`        | Revert previous agent changes.         |
| `/redo`        | Restore reverted changes.              |
| `/diff`        | Show all changes made in the session.  |
| `/checkpoint`  | Save current working state.            |
| `/restore`     | Restore a previous checkpoint.         |
| `/resume`      | Resume an earlier conversation.        |
| `/sessions`    | Browse previous sessions.              |
| `/clear`       | Start a fresh conversation.            |
| `/compact`     | Manually compact context.              |
| `/plan`        | Enter planning mode.                   |
| `/review`      | Review changes without modifying them. |
| `/permissions` | Configure tool permissions.            |
| `/sandbox`     | Configure execution boundaries.        |
| `/export`      | Export conversation or session.        |
| `/init`        | Generate project instructions.         |

I'd also include a permission system:

```
Read files             Allowed
Edit files             Ask
Execute shell          Ask
Delete files           Ask
Web access             Allowed
Spawn subagents        Allowed
```

And a configurable policy:

- Always allow
- Ask every time
- Allow for this session
- Allow for this project
- Deny

# 10. My recommended architecture for YopesCLI

[Inside Claude Code: The Architecture That Makes AI Actually Do the Work](https://images.openai.com/static-rsc-4/Xev3_jJ0ytcYns-q8XufqLIynAuZqdMcnEDB6FskY4diXO4_oz95r7ZNZFulz2-UbDGUF7mR9lEFexdcyvKsreIN_hU7pfsKRIm_vWWxaOhc-w7GnMklkvSPM9NZR_F0uH9Wum6eQTBfhGWofshYb2HZoQpO9InuHW2ixtV34Uc?purpose=inline)

[qubytes.substack.com](https://qubytes.substack.com/p/claude-code-architecture-explained?utm_source=chatgpt.com)

I'd organize it into these subsystems:

```
YopesCLI
│
├── Frontend
│   ├── Terminal UI
│   ├── Diff Viewer
│   ├── Task UI
│   ├── Image Preview
│   └── Input / Clipboard
│
├── Agent Runtime
│   ├── Tool Calling
│   ├── Planning
│   ├── Agent Loop
│   ├── Context Manager
│   ├── Subagents
│   └── Model Router
│
├── Tools
│   ├── Read
│   ├── Edit
│   ├── MultiEdit
│   ├── Write
│   ├── Bash
│   ├── Glob
│   ├── Grep
│   ├── Find
│   ├── Web Search
│   ├── Web Fetch
│   ├── Diagnostics
│   └── Symbols
│
├── State
│   ├── Sessions
│   ├── Tasks
│   ├── Checkpoints
│   ├── Memory
│   └── Tool Permissions
│
└── Providers
    ├── OpenAI
    ├── Anthropic
    ├── Google
    ├── OpenAI-compatible
    └── Custom Providers
```

# 11. The feature I'd consider your biggest differentiator

A proper live agent dashboard inside the terminal.

Not just text scrolling.

Something like:

[GitHub - richer-richard/fenestra: Pure-Rust native GUI framework with web-grade aesthetics and first-class headless PNG rendering — winit + wgpu + vello + parley + taffy · GitHub](https://images.openai.com/static-rsc-4/GDA6IYY2_ymRnrNy_sCQR2J5IMoJ0lY_dWJikMehl08krk46reMlSgbJXJFORNDd3XH2ywqfYIIfpmK0I1Dmb6YPT_-DVAW1LAfVolrnnYR2sraPIRvZCt0PdFw46_BSctVLldFqDDqHBfF26B7jjInRtbbajD2Z_EuxbaGb2x8?purpose=inline)

[github.com](https://github.com/richer-richard/fenestra/?utm_source=chatgpt.com)

```
┌ YopesCLI ───────────────────────────────────────────────────────┐
│ Model: GLM | Effort: High | Context: 42% | Agents: 3           │
├────────────────────────────────────────────────────────────────┤
│                                                                │
│ Main Agent                                                     │
│   Implementing authentication                                 │
│                                                                │
│ ┌ Agents ───────────────────────────────────────────────────┐   │
│ │ Agent 1  Analyzing database          ✓ Completed          │   │
│ │ Agent 2  Writing unit tests          ● Running             │   │
│ │ Agent 3  Reviewing security           ● Running             │   │
│ └───────────────────────────────────────────────────────────┘   │
│                                                                │
│ ┌ Tasks ────────────────────────────────────────────────────┐   │
│ │ ✓ Inspect project                                         │   │
│ │ ✓ Design architecture                                     │   │
│ │ ● Implement authentication                                 │   │
│ │ ○ Add tests                                               │   │
│ └───────────────────────────────────────────────────────────┘   │
│                                                                │
├────────────────────────────────────────────────────────────────┤
│ > Implement JWT authentication                                │
└────────────────────────────────────────────────────────────────┘
```

That would make YopesCLI feel like an actual agent environment rather than just a chat interface connected to tools.

## My implementation priority

If I were building this, I'd implement it in this order:

| Priority | Features                                                        |
| -------- | --------------------------------------------------------------- |
| P0       | Bash, Read, Edit, Write, Glob, Grep, Diff Viewer                |
| P1       | MultiEdit, Permissions, TODO UI, Clipboard, Image Paste         |
| P2       | Web Search, Web Fetch, Context Management, Sessions             |
| P3       | Subagents, Background Processes, Diagnostics                    |
| P4       | Live Dashboard, Checkpoints, Symbol Navigation, Advanced Memory |

One final thing: make the harness model-agnostic. Tool definitions, agent orchestration, context management, and UI should all work independently of which model you're using. That way you can switch between GLM, Claude, GPT, or other compatible models without rebuilding the harness.

If you want, I can turn your entire YopesCLI feature list into a proper AI harness specification with tool schemas, slash commands, UI behavior, permission rules, and an implementation roadmap.
