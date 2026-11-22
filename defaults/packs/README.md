# Guideline packs

The format of a guideline pack: the files in its directory, the fields of its `pack.yaml`, its templates, and the header of the copy a repository adapts from it.

## Layout

- This file, `defaults/packs/README.md`, is the definition of the format and is not a pack.
- Every directory `defaults/packs/<id>/` is one pack and holds exactly these:
  - `pack.yaml`, the manifest;
  - `conventions.md`, the conventions template, required under that fixed name. It is Markdown whose first line is a `# ` title, and it never starts with the header of an adapted copy;
  - `settings/`, the settings templates (subdirectories are allowed), present exactly when `settings` is non-empty. Each file in it is referenced by at least one `settings` entry.
- A pack has no other files.
- Every file is UTF-8 without a BOM, with LF line endings.

## `pack.yaml`

### YAML rules

- It must load the same under mikefarah yq (`yq -p=yaml -o=json .`) and PyYAML `safe_load`:
  - one document, with a mapping at the top level;
  - no `---` or `...` lines;
  - no anchors, aliases, merge keys (`<<`) or tags;
  - no duplicate keys in any mapping. Loaders silently keep the last one, so this is a review rule.
- Every string value is quoted with `"…"` or `'…'`, on one line, with no block scalars. Quoting is what keeps `"3.10"`, `"yes"`, `"010"`, `"2026-11-22"` and `"{{ package_root }}/x"` as strings.
- In a double-quoted string the only escapes are `\\` and `\"`. A string that needs a backslash, such as a `pattern`, is better single-quoted.
- Integers are plain decimal: `0` or no leading zero, and no `+`, `_` or exponent. Every integer lies between -2147483647 and 2147483647.
- Booleans are `true` or `false`. A boolean is not an integer and an integer is not a boolean, so a Python checker tests `type(value) is int`, not `isinstance`.
- No floats and no `null` anywhere. An optional field that is left out takes its default.
- A key not in the tables below, or a value of the wrong type, is an error.

### Top-level fields

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `id` | string fully matching `[a-z][a-z0-9-]*`, equal to the directory name | yes | none |
| `version` | integer of at least 1 | yes | none |
| `name` | string, the display name (for example `"TypeScript"`) | yes | none |
| `description` | string | no | `""` |
| `base` | string, the id of another pack (see [Base packs](#base-packs)) | no | absent |
| `detect` | mapping (see [`detect`](#detect)) | yes | none |
| `parameters` | list of parameter entries | no | `[]` |
| `settings` | list of settings entries | no | `[]` |
| `gates` | list of gate entries | no | `[]` |

### `detect`

Parameters are not allowed in `detect`.

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `files` | list of strings, globs over tracked paths | no | `[]` |
| `shebangs` | list of strings, interpreter names | no | `[]` |

- The two lists hold at least one entry between them.
- Globs have the semantics of `.mintswarm/dogfood/plan-approval/floor.py`: a glob matches the whole path; `**` matches anything including `/`, `**/` zero or more directories, `*` anything within one segment, `?` one character other than `/`; everything else is literal. So a pack writes `**/*.py`, not `*.py`.
- The interpreter of a tracked file is read from its first line as bytes, without the trailing LF and then one trailing CR:
  - the line starts with `#!`; the rest is decoded as UTF-8, and a decoding failure means no interpreter;
  - the rest is split on ASCII spaces and tabs, dropping empty words; with no word there is no interpreter;
  - the interpreter is the basename of the first word. If that basename is `env`, it is the first later word that neither starts with `-` nor contains `=`, and with no such word there is no interpreter.
- A pack is detected when any of its globs matches a tracked path, or when a listed interpreter is the interpreter of any tracked file.

### Parameter entry

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `name` | string fully matching `[a-z][a-z0-9_]*`, unique in the pack | yes | none |
| `type` | `"string"`, `"integer"` or `"boolean"` | yes | none |
| `default` | a value of `type` | yes | none |
| `description` | string, what the setup session asks the maintainer | yes | none |
| `choices` | non-empty list of values of `type`, with no repeats, containing `default`; not allowed for `boolean` | no | absent (any value of the type) |
| `pattern` | string, a Python regular expression that every value must fully match (`re.fullmatch`); only for `"string"` | no | absent |

- The values of a parameter are its `default`, its `choices` and any value chosen at adoption. Every string value obeys these rules:
  - it is one line with no control characters: none of Unicode categories Cc and Cf, nor U+2028 or U+2029;
  - it fully matches `pattern`, if given;
  - if the parameter appears in a `target`, `alternatives` entry or `gates[].command`, it fully matches `[A-Za-z0-9._/+-]+` and does not start with `-`.
- A template author places a string value in `conventions.md` or a settings template only where every value those rules allow is safe. A string parameter that appears inside a quoted string of a settings template (JSON, TOML, YAML, shell) declares a `pattern` that excludes that quote and the backslash, and in a shell double-quoted string also `$` and the backtick.

### Settings entry

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `template` | string, a path relative to the pack directory, starting with `settings/`, of an existing file | yes | none |
| `target` | string, a repository path; may contain parameters | yes | none |
| `kind` | `"formatter"`, `"linter"`, `"compiler"`, `"toolchain"` or `"editor"` | yes | none |
| `mode` | `"file"` or `"fragment"` | no | `"file"` |
| `section` | string, the section the fragment provides (for example `"tool.black"`, `"workspace.lints"`, `"*.sh"`) | with `"fragment"`; not allowed with `"file"` | absent |
| `optional` | boolean; `true` means the setup session only proposes the setting | no | `false` |
| `alternatives` | list of strings, globs over tracked paths; may contain parameters. A match means the tool is already set up elsewhere (for example `".prettierrc*"` or `"setup.cfg"`) | no | `[]` |

- **Paths:**
  - In `target` and `alternatives`, a placeholder must be a whole directory segment followed by `/`. After rendering, `.` segments are dropped, so the default `"."` gives a path at the repository root.
  - The whole rendered `target`, each rendered `alternatives` entry and `template` are non-empty and relative (no leading `/`), with no empty, `..` or `.git` segment (`.git` compared without regard to case) and no `\`.
  - `target` and `template` also contain none of `*`, `?`, `[` or `]`. `template` has no `.` segment.
  - A consumer without the values, such as the readiness check, reads each placeholder segment as `**`. For example, `{{ package_root }}/tsconfig.json` checks `**/tsconfig.json`.
  - The setup session refuses to write a target when the target or any of its existing parent directories is a symlink.
- **`file` mode:** the rendered template is the whole target file.
- **`fragment` mode:**
  - The target is a TOML, EditorConfig or INI-style file with bracketed section headers.
  - A header line is a line that, with surrounding whitespace stripped, fully matches `\[ *([^\[\]#]+?) *\] *(#.*)?`; group 1 is the header's name.
  - The template starts with a header line. Every header in it is named `<section>` or `<section>.<rest>` with a non-empty `<rest>`, and no two have the same name. `[[…]]` headers are not allowed.
  - A template header is present when the target has a header line with the same name. The fragment is present when all its headers are.
  - Merging: if the target does not exist, the session writes the rendered fragment as the file. Otherwise it keeps every present header and its lines as they are, and appends each missing header with its lines, up to the template's next header line.
  - A section the target defines in another form (quoted keys, dotted keys, an inline table) counts as not present. The session does not append to such a target; it reports the conflict to the maintainer.
- The pair of rendered `target` and `section` is unique within a pack, and a `file` entry shares its rendered target with no other entry. Both are checked rendered with defaults and again with the values chosen at adoption.
- A setting counts as configured when its target exists (for a fragment, with the fragment present) or when any of its alternatives matches a tracked path.

### Gate entry

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `name` | string fully matching `<id>-[a-z0-9][a-z0-9-]*`, unique in the pack | yes | none |
| `command` | string; may contain parameters; once rendered, `shlex.split` gives at least one word | yes | none |
| `kind` | `"format"`, `"lint"`, `"typecheck"` or `"test"` | yes | none |
| `timeout` | integer, seconds, at least 1 | no | absent (the runner's default, 1800) |
| `optional` | boolean; `true` means the setup session only proposes the gate | no | `false` |

- A gate is copied into the repository's `gates.run` as `name`, the rendered `command`, and `timeout` if given. That is the shape `.mintswarm/dogfood/run-gates.py` and `.mintswarm/dogfood/plan-approval/floor.py` read. `kind` and `optional` stay in the pack.
- A `gates.run` entry counts as the pack's gate when its `name` is the same.

## Templates

- A placeholder is a match of `\{\{ *([a-z][a-z0-9_]*) *\}\}`: spaces only, never a newline.
- Placeholders are allowed in `conventions.md`, in every file under `settings/`, and in `settings[].target`, `settings[].alternatives` and `gates[].command`. They are allowed nowhere else, including `detect` and `section`.
- **Rendering:**
  - One pass over the whole text with a function as the replacement, never a replacement string: `pattern.sub(lambda m: text[m.group(1)], template)`, where `text` maps each name to its value's text. Replaced text is never scanned again.
  - A string value renders verbatim, an integer in decimal, a boolean as `true` or `false`.
  - "Rendered with defaults" means every parameter takes its `default`.
- **Undeclared names and literal text:**
  - A match whose name is not a parameter declared in the same pack is an error. So `${{ false }}` in a GitHub Actions workflow template is an error; write `${{ !true }}`.
  - Text that does not match is literal, for example `${{ secrets.TOKEN }}`, `{{- x }}` and `{{ Name }}`.
  - There is no escape syntax, so a template cannot print a placeholder literally.
- Every declared parameter appears in at least one placeholder.
- There are no transforms on values. A derived form is a separate parameter, for example major and minor integers rendered as `py{{ python_major }}{{ python_minor }}`.

## Base packs

- `base` names an existing pack other than the pack itself, and that pack has no `base` of its own. Nesting is limited to one level, so there are no cycles.
- A derived pack:
  - uses only its own parameters in its templates;
  - does not repeat its base's conventions;
  - is adopted only together with its base.
- A derived pack adds its own settings files and never replaces a base entry. Where a tool reads one configuration, the base template takes in the derived file when it is present (for example, an ESLint configuration that imports an optional extra file).
- The combined settings are the base's entries followed by the derived pack's, and the uniqueness rules for settings apply to the combined list. The combined gates are the base's followed by the derived pack's.
- Each adapted copy's header names only its own pack. A newer base does not change the derived pack's version.

## Adapted copy

- A repository's adapted copy of a pack lives at `docs/conventions/<id>.md`, where `<id>` is the pack id.
- Line 1 is the header, and the rendered `conventions.md` starts on line 2.

### Header

- The header is exactly `<!-- mintswarm-pack: <id> <version> -->` followed by LF, with single spaces and no trailing whitespace.
- Whoever makes the copy writes the header; it is never stored in the pack.
- The id and version are read with this regular expression, where `first_line` is the file's first line without its `\n`; group 1 is the id and group 2 the version:

  ```python
  re.fullmatch(r"<!-- mintswarm-pack: ([a-z][a-z0-9-]*) ([1-9][0-9]*) -->", first_line)
  ```

- A first line that, after removing a leading BOM, does not start with `<!-- mintswarm-pack:` means there is no header: the file is a hand-written document, not a pack copy.
- A first line that, after removing a leading BOM, starts with `<!-- mintswarm-pack:` but whose `first_line` does not match is a malformed header. A copy keeps LF line endings and no BOM, so a header after a BOM or ending in CR is malformed.
- A matching header is also malformed when its id differs from the file name without `.md`, or when its version is higher than the pack's.
- Example: `<!-- mintswarm-pack: sample 2 -->` reads as id `sample`, version `2`.

## Versioning and copies

- `version` starts at 1.
- A PR that changes any file under `defaults/packs/<id>/` raises that pack's `version` by exactly one. A change to this README changes no pack.
- A repository may edit its copy and its settings freely. The header keeps the version the copy was adapted from.
- A newer version of a pack reaches a repository only as a PR that updates the copy and its header, never by a silent write. A copy whose version is lower than the pack's is not current.

## Example

A made-up pack, `sample`, at version 2. It exists only in this README; there is no `defaults/packs/sample/` directory.

`defaults/packs/sample/pack.yaml`:

```yaml
id: "sample"
version: 2
name: "Sample"
description: "A made-up language, used only to show the format."
detect:
  files: ["**/*.smp"]
  shebangs: ["sample"]
parameters:
  - name: "min_major"
    type: "integer"
    default: 3
    description: "Which major version of Sample is the oldest the code supports?"
  - name: "min_minor"
    type: "integer"
    default: 9
    description: "Which minor version of that major version is the oldest the code supports?"
  - name: "line_length"
    type: "integer"
    default: 120
    description: "What is the longest line the formatter allows?"
    choices: [80, 100, 120]
  - name: "package_root"
    type: "string"
    default: "."
    description: "Which directory, relative to the repository root, holds the Sample package?"
settings:
  - template: "settings/pyproject.toml"
    target: "{{ package_root }}/pyproject.toml"
    kind: "formatter"
    mode: "fragment"
    section: "tool.sample"
  - template: "settings/sample-lint.toml"
    target: "{{ package_root }}/.sample-lint.toml"
    kind: "linter"
    optional: true
    alternatives: ["{{ package_root }}/setup.cfg"]
gates:
  - name: "sample-format"
    command: "sample fmt --check {{ package_root }}"
    kind: "format"
  - name: "sample-lint"
    command: "sample lint {{ package_root }}"
    kind: "lint"
    timeout: 600
```

`defaults/packs/sample/settings/pyproject.toml`, the fragment template:

```toml
[tool.sample]
line-length = {{ line_length }}
target-version = "{{ min_major }}.{{ min_minor }}"
```

Rendered with defaults, its target is `pyproject.toml` at the repository root, and the fragment counts as present when that file has a `[tool.sample]` header line. A readiness check without the values looks for `**/pyproject.toml`.

The opening of `defaults/packs/sample/conventions.md`:

```markdown
# Sample conventions

Code supports Sample {{ min_major }}.{{ min_minor }} and later.
```

The first two lines of the adapted copy, `docs/conventions/sample.md`, rendered with defaults:

```markdown
<!-- mintswarm-pack: sample 2 -->
# Sample conventions
```

When adopted, the copied `gates.run` entries are `sample-format` with the command `sample fmt --check .`, and `sample-lint` with the command `sample lint .` and a timeout of 600.
