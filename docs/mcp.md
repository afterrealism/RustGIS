# MCP Server

`rustgis-cli mcp` runs RustGIS as a [Model Context Protocol](https://modelcontextprotocol.io)
server, so an AI agent such as Claude Code, Claude Desktop, Cursor, VS Code or Codex can make
maps with it. The agent opens data, runs any RustGIS command or geoprocessing tool, reads
attribute tables, looks at the map as an image and looks things up in this documentation.

```sh
rustgis-cli mcp --sample                # the World Atlas sample
rustgis-cli mcp --data parcels.geojson  # your own data (--data more than once, or --project p.rgis)
rustgis-cli mcp --yes                   # also allow file and network access
rustgis-cli mcp --attach [--yes]        # the desktop app that is already open (its MCP Relay)
```

The server talks over standard input and output, so you don't start it yourself: you tell your
agent how to start it (below), and the agent runs it. Each agent gets its own project, held in
memory until the agent saves it.

## Adding it to an agent

Claude Code:

```sh
claude mcp add rustgis -- rustgis-cli mcp --sample
```

Codex:

```sh
codex mcp add rustgis -- rustgis-cli mcp --sample
```

Claude Desktop (`claude_desktop_config.json`), Cursor (`~/.cursor/mcp.json`) and other clients
that use an `mcpServers` list:

```json
{
  "mcpServers": {
    "rustgis": {
      "command": "rustgis-cli",
      "args": ["mcp", "--sample"]
    }
  }
}
```

VS Code (`.vscode/mcp.json`):

```json
{
  "servers": {
    "rustgis": {
      "type": "stdio",
      "command": "rustgis-cli",
      "args": ["mcp", "--sample"]
    }
  }
}
```

If the agent can't find `rustgis-cli`, give its full path (see [Installation](installation.md)).
Then ask, for example:

- "Which 5 countries have the largest GDP? Show them on the map."
- "Buffer the cities by 200 km, colour the result by population and show me the map."
- "Load ~/data/parcels.geojson, label the parcels with their owner and export the map to
  parcels.png." (this reads and writes files, so it needs `--yes`)

## Managing the open desktop app

By default each agent gets its own project in memory. To drive the project that is open in the
desktop app instead, run the **MCP Relay** command (Help menu, or Command Search with Ctrl+K).
The relay is off until that command runs. While it runs, the status bar shows `MCP relay · n attached`,
where n is the number of authenticated clients; hovering shows `Stop MCP relay`, and clicking
it stops the relay.

```sh
rustgis-cli mcp --attach                 # the open app on 127.0.0.1:8767
rustgis-cli mcp --attach 9000 --yes      # another port, with file and network access
```

```json
{
  "mcpServers": {
    "rustgis-app": {
      "command": "rustgis-cli",
      "args": ["mcp", "--attach", "--yes"]
    }
  }
}
```

Attached calls run on the open app's session through the same tools and refusal policy as the
private server; file and network commands still need `--yes`.

### Relay requirements

- **Off until asked.** Only `ui.mcpRelay` starts a listener. `ui.mcpRelay` with no parameters
  or with the same port reports the address it already listens on. A different explicit `port`
  is refused until the relay is stopped. `ui.mcpRelay {"stop": true}` stops it and is
  idempotent. The default port is 8767.
- **Loopback only.** `Relay::bind` refuses any non-loopback address before a socket is opened; the
  app's `ui.mcpRelay` binds 127.0.0.1.
- **Private token.** Each run draws 16 bytes from the operating system's random source,
  written as 32 hex characters. The token file is `{port}.token` in `$XDG_RUNTIME_DIR/rustgis`
  when that is an absolute folder, otherwise in a per-user folder under the system temporary
  folder. The folder must be owned by the current user and not accessible to others. The file
  must be a regular file owned by the current user, without group or other access. A file
  that is a symlink is refused. `rustgis-cli mcp --attach` reads it with `read_token(port)`.
  The token never appears on a command line. The file is removed when the relay stops.
- **Attach first.** A connection's first non-empty line must be a `rustgis/attach` message
  carrying the token. It must arrive within 5 seconds, measured from connection, and be at most
  4096 bytes including its newline. The token is compared without early exit. A connection that fails
  this is closed. The `access` flag sets what that connection may do; the per-call refusal
  policy still applies. A repeat attach may change `access`.
- **Handshake and idle limits.** Attached clients that send nothing for 300 seconds are closed.
  A reply write that takes longer than 10 seconds closes the connection.
- **Connection caps.** At most 16 open connections, and at most 4 that have not attached. A
  connection beyond either cap is dropped without a reply.
- **Queue cap.** At most 64 messages wait for the app's frame. A request beyond that gets error
  `-32001` (`the app is busy; try again`) if it expects a reply.
- **Bounded request waits.** A request waits up to 60 seconds for the app to run it. If it is
  still pending then, it is cancelled and never runs; the client gets error `-32002`. If it is
  already running, the client waits one more 60-second interval. If the reply still has not
  come, the client gets error `-32003` (`its outcome is unknown`), and the command may still
  complete later.
- **Stop.** Stopping the relay, or dropping its last owner, closes the listener and every
  client, drops queued messages unrun, and removes the token file. A waiting request ends at
  once, checked every 25 milliseconds.
- **Wake.** A queued message or a change in the attached-client count wakes the UI thread so
  it draws a frame. A relay started before the first frame installs its wake on the first
  logic call after that frame exists.
- **Shared framing.** Each message is one JSON-RPC message per line. After attach, messages and
  replies are read with `read_message_max` at 64 MiB (`MAX_LINE`), the limit counting the newline.
  The attach line is read at most 4096 bytes including its newline (see Attach first). After an
  oversized line on a socket the relay sends a -32700 error and closes that connection. On standard
  input the reader skips the rest of the line and continues. IO errors name neither standard input
  nor a socket.

### Tests

These behavioral tests in `crates/assistant/src/relay.rs` check the requirements above:

- Loopback bind and command run: `relay_runs_a_command_on_the_target_session`
- Token required: `relay_drops_connections_without_the_token`
- Parse error after attach: `relay_returns_parse_error_after_attach_and_stays_usable`
- File commands need `--yes`: `relay_refuses_file_commands_without_yes`
- Timed-out command never runs: `relay_cancels_a_timed_out_command_so_it_never_runs`
- Stop cleanup: `relay_stop_closes_clients_and_removes_the_token`
- Last owner drop: `relay_last_owner_drop_closes_the_listener`
- Unauthenticated cap: `relay_caps_unauthenticated_connections`
- Oversized pre-attach line: `relay_refuses_an_oversized_unauthenticated_line`
- Token random and private: `relay_token_is_random_and_private`
- Client count and wake: `relay_counts_authenticated_clients_and_wakes_on_change`
- Stop ends a reply wait: `relay_stop_ends_a_reply_wait_and_the_command_never_runs`
- Running command reported as unknown: `relay_reports_a_running_command_past_its_wait_as_unknown`
- Handshake deadline: `relay_closes_a_trickling_handshake_at_its_deadline`


## Tools

| Tool | What it does |
| --- | --- |
| `list_layers` | The map: its layers (name, type, feature count, fields, visibility, selection), basemap, centre and scale |
| `view_map` | The map as a PNG image, at the current view |
| `search_commands` | Finds RustGIS commands by words: their ids, ribbon locations and parameters |
| `run_command` | Runs a command by id with JSON parameters; each run is one undo step. The result names the layers it added (`layersAdded`) and what the command said beside it (`notices`), including any parameters it ignored (`layer.moveDown ignores 'steps' (it reads layer)`) |
| `search_tools` | Finds geoprocessing tools by words |
| `describe_tool` | A geoprocessing tool's parameters |
| `run_tool` | Runs a geoprocessing tool; its output is added to the map as a new layer (with `layersAdded` and `notices` as for `run_command`) |
| `read_features` | A layer's attribute rows, optionally only those matching a where clause, sorted |
| `search_docs` | Searches this documentation and returns the best matching sections |
| `read_doc` | Reads a documentation page or section as Markdown; `offset` (1-based line) and `limit` (default 2000 lines) page through long results using `next_offset` |

These are the [AI Assistant](user-guide/assistant.md)'s tools, plus `view_map`. Every page of this documentation is also a resource
(`rustgis://docs/cli.md`, `rustgis://docs/user-guide/maps.md`…) that a client can attach to a
conversation.

## What the agent may do

Your agent asks you before each tool call, and the server adds its own limits:

- Commands that read or write files, connect to an address on the internet or run a plugin's
  command run only when the server was started with `--yes`, as does opening a project from
  text (it could bring in web layers). Without it, the agent can still work with the data you
  gave the server and with the built-in sample, basemaps and geoprocessing. `view_map` draws
  the web tile layers the project you opened already has, fetching their tiles.
- Plugin management, scripts, tasks, models and the GeoLibre runner are never run from here.
- In private-server mode, opening, replacing and undoing in the project are allowed: the project
  is the agent's own. With `--attach`, these operations affect the project open in the desktop app.

Installed [plugins](plugins.md) load first, as with the other subcommands (`--no-plugins` skips
them). Notes such as "MCP server ready" go to standard error; standard output carries only the
protocol.
