# Connect an agent

Any MCP client can drive Cutweave: it starts `cutweave mcp` as a stdio MCP server (WP-1J.03). Architecture: [§14](../01-architecture.md#14-mcp-server-in-cutweave-api) and [ADR-0009](../adr/0009-mcp-server.md); every flag is in [cli.md](cli.md#cutweave-mcp).

## Attach or headless

| Mode | Command | Use it when |
|---|---|---|
| **Attach** | `cutweave mcp --attach` | The desktop app (or `cutweave engine`) is running. The agent edits the project you have open and you watch it happen; its changes are in the same undo history, as `agent:mcp`. |
| **Headless** | `cutweave mcp --headless --allow-root <folder>` | No GUI: CI, servers, scripted runs. The engine runs inside the MCP server process. |

- **Attach** finds the running engine through its run-file in `~/.cutweave/run`. With several engines running, add `--pid <pid>`; `--run-file <path>` names one directly.
- **Headless** confines the agent to the `--allow-root` folders and the project folder (`--project <folder>` opens one at start); paths elsewhere fail with `invalid`. Without `--allow-root` or `--project` the agent cannot import media or save a project. Until the FFmpeg backend lands, add `--dev-stub-media` to import files with made-up probe data (dev only).
- **Profiles:** `--profile minimal` lists at most 12 tools for small context windows; `standard` (the default) lists the core editing tools plus `cutweave_search_commands`, `cutweave_describe_command`, `cutweave_execute` and `cutweave_query`, which reach every other command.

`cutweave` must be on your `PATH`; otherwise put the binary's absolute path in `command` (a dev build is `target/debug/cutweave`).

## Claude Code

```sh
claude mcp add cutweave -- cutweave mcp --attach
claude mcp add cutweave-headless -- cutweave mcp --headless --allow-root /path/to/footage
```

`--scope project` writes the server to the repository's `.mcp.json` instead, for everyone working on it.

## Claude Desktop

`claude_desktop_config.json` (Settings, Developer, Edit Config):

```json
{
  "mcpServers": {
    "cutweave": { "command": "cutweave", "args": ["mcp", "--attach"] }
  }
}
```

## Codex

`~/.codex/config.toml`:

```toml
[mcp_servers.cutweave]
command = "cutweave"
args = ["mcp", "--attach"]
```

## Gemini CLI

`~/.gemini/settings.json`, or `.gemini/settings.json` in a project:

```json
{
  "mcpServers": {
    "cutweave": { "command": "cutweave", "args": ["mcp", "--attach"] }
  }
}
```

## Cursor

`~/.cursor/mcp.json`, or `.cursor/mcp.json` in a project:

```json
{
  "mcpServers": {
    "cutweave": { "command": "cutweave", "args": ["mcp", "--attach"] }
  }
}
```

## Headless in any client

Use the same entry with the headless arguments, for example:

```json
{
  "mcpServers": {
    "cutweave": {
      "command": "cutweave",
      "args": ["mcp", "--headless", "--allow-root", "/path/to/footage"]
    }
  }
}
```

On Windows, escape backslashes in JSON paths (`"C:\\Users\\me\\Videos"`).

## What to expect

- **Revisions:** both MCP revisions work, 2025-11-25 (`initialize`) and 2026-07-28 (`server/discover`, no handshake); the client picks. Attached, the proxy passes every message through unchanged, so the agent negotiates with the engine itself.
- **Sessions:** one `cutweave mcp` process is one session. When the client stops the server (it closes stdin), an edit transaction the agent left open is rolled back. Attached over 2026-07-28 every request is independent, so no transaction outlives a request.
- **Engine gone:** if the attached engine quits or crashes, calls in flight fail with an error whose `data.code` is `connection_lost`, and `cutweave mcp` exits with code 3. Start Cutweave again, then reconnect the server (in Claude Code: `/mcp`).
- **Errors before the session** (no engine running, several engines without `--pid`, bad flags) are one JSON object on stderr; most clients show it in their MCP log. Exit codes: 2 usage, 3 no engine, 1 the headless engine did not start (for example `project_locked`).
- **Logs:** stdout carries protocol messages only. A headless engine logs to `~/.cutweave/logs/engine-<pid>.log` (`--log-dir`, `--no-log`); an attached engine logs in its own process.
