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
    description = "Adds an external research command"
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

Supported capabilities are metadata-provider, commands, activity-events, and
read-paper-metadata. Supported response actions are notify and
add_to_collection.

Plugins must treat requests as independent and should finish quickly. The host
enforces an invocation timeout and rejects output larger than 1 MiB.
