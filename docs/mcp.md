# Running Azimut from an agent (MCP)

How to let Claude Code, Codex or another MCP client run Azimut for you, asking at each decision.

`azimut mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/) server on stdio,
so that an agent such as Claude Code or Codex can run Azimut for a user, in the user's
project folder. Add it once per project:

```sh
claude mcp add azimut -- azimut mcp          # Claude Code
codex mcp add azimut -- azimut mcp           # Codex
```

or share a `.mcp.json` in the project (set `AZIMUT_RTKLIB_DIR` when RTKLIB-EX is not on the
PATH):

```json
{ "mcpServers": { "azimut": { "command": "azimut", "args": ["mcp"],
  "env": { "AZIMUT_RTKLIB_DIR": "/path/to/rtklib/bin" } } } }
```

Then ask, for example: "Do the PPK of the flights in `field/2024-08-09` with the base in
`base/`, coordinates from `base/12345678.sum`, with CGVD2013 heights."

| Tool | Does |
|---|---|
| `scan_folder` | Inventory and diagnosis (read only). |
| `read_ppp_summary` | Position, systems, uncertainty and point of a CSRS-PPP summary (read only). |
| `process_ppk` | PPK; writes the outputs and report, returns the report summary. Images untouched. |
| `shift_rtk` | RTK correction; same outputs. |
| `write_images` | Positions into the images, in place or into copies; dry run unless `confirm: true`. |
| `restore_images` | Undo an in-place write. |
| `read_report` | Full report of a flight. |

The server never takes a decision that is the user's. When the base files, the base
coordinates, their frame, the shift, the image mode or the consent to write images are
missing, the tool does nothing and returns `status: "needs_decision"` (or
`"needs_confirmation"` after a dry run) with the questions for the agent to ask. Coordinates
are never accepted without a frame. The server also gives the agent working instructions and
a `ppk_workflow` prompt (step by step, asking at each decision). Long runs send progress
notifications when the client asks for them; if the client times out tool calls, raise its
limit (Claude Code: `MCP_TOOL_TIMEOUT`).

---
[Back to the README](../README.md)
