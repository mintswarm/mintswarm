# Result-block fixtures

Each directory is one case: `block.yaml` holds a result block in the JSON-in-YAML form, and `result.json` holds what it means.

`result.json` is the `msw-result` mapping in its one JSON form, as `StageOutcome` writes it: every key is present, optional fields the block leaves out or sets to `null` are `null`, and absent or `null` lists are `[]`. The raw YAML tree of a block therefore equals `result.json` only after those defaults are applied. Every reader of these fixtures, in Rust or Python, applies them before it compares.
