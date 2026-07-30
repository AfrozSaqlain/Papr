# Plugin protocol

Plugin API version: 1.

Each plugin lives in its own directory beneath the plugins path printed by
papr paths:

    plugins/
      example/
        plugin.toml
        example-plugin

Example manifest:

    id = "example"
    name = "Example Provider"
    version = "1.0.0"
    api_version = 1
    description = "Adds an external integration"
    executable = "example-plugin"
    args = []
    capabilities = ["commands", "read-paper-metadata"]

The executable path must remain inside the bundle. A discovered plugin is
disabled until its ID is added to enabled_plugins in config.toml:

    enabled_plugins = ["example"]

## Transport

papr starts one process per invocation, writes one JSON request to standard
input, closes input, and reads one JSON response from standard output.
Diagnostics belong on standard error.

Request:

    {"api_version":1,"event":"command.run","context":{"paper_id":42}}

Response:

    {"actions":[{"type":"notify","message":"Command completed"}]}

Capability names are metadata-provider, commands, activity-events, and
read-paper-metadata. They describe the integration surface a bundle intends
to use and are displayed by Papr; the manifest does not grant implicit access.
Supported response actions are notify and add_to_collection (assign the paper
in the request context to a Group).

Papr currently sends lifecycle events named paper_imported, paper_downloaded,
and paper_opened for local papers. The built-in auto-tagger listens to the
first two. A bundle is only executable when its ID appears in enabled_plugins;
`papr plugins` lists discovered bundles and validation diagnostics. `papr
plugin <ID> <EVENT> [--timeout <seconds>]` invokes an enabled plugin manually
with an empty JSON context.

Plugins must treat requests as independent and should finish quickly. The host
enforces an invocation timeout and rejects output larger than 1 MiB.
