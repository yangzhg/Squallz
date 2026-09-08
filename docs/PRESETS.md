# Archive Presets

Archive presets are versioned policy snapshots shared by the desktop app and
file-manager entry points. They contain reusable archive settings, never the
files or folders selected for a particular job.

## Storage

The preset document is stored beside the other Squallz configuration files:

- macOS: `~/Library/Application Support/Squallz/presets.json`
- Windows: `%APPDATA%\Squallz\presets.json`
- Linux: `$XDG_CONFIG_HOME/Squallz/presets.json` (normally
  `~/.config/Squallz/presets.json`)

`schema_version` identifies the current document contract and `revision` is
used for compare-and-swap updates. Squallz validates the complete replacement
before it changes the file. Writes use a private same-directory temporary file, an
inter-process lock and a durable replacement. A corrupt document or an unknown
schema version is reported and left untouched.

## CLI management

The CLI manages the same document as the desktop app. It never writes a second
CLI-only preset store:

```sh
sqz preset list
sqz preset list --kind create --json
sqz preset show builtin.create.cross-platform-7z --json
sqz preset clone builtin.create.cross-platform-7z user.create.portable \
  --label "Portable"
sqz preset bind app-create user.create.portable
sqz preset unbind file-manager-create
sqz preset path
```

`clone` is the safe starting point for a new preset because it copies a
currently valid create or extract policy and always marks the result editable.
To change the complete policy, export that one object, edit it, and write it
back:

```sh
sqz preset show user.create.portable --json > portable.json
sqz preset update user.create.portable --file portable.json
```

The JSON object must keep the same `id`, the same create/extract kind and
`"built_in": false`. Input is limited to 1 MiB. `update`, `delete`, `bind` and
`unbind` load the current revision and commit through the core preset store's
compare-and-swap validation, inter-process lock and atomic replacement. A
concurrent edit fails instead of overwriting the newer document. Built-in
presets cannot be updated or deleted. Deleting a bound custom preset restores
the corresponding safe built-in binding; `unbind` is the explicit way to leave
a slot empty. File-manager bindings must satisfy the stricter preset-kind rules
described below.

## Current contract

A document contains named create and extract presets plus four optional
bindings:

```json
{
  "schema_version": 1,
  "revision": 0,
  "presets": [
    {
      "kind": "create",
      "id": "builtin.create.cross-platform-7z",
      "label": "Cross-platform 7Z",
      "built_in": true,
      "options": {
        "format": "7z",
        "level": 5,
        "credential": { "kind": "none" },
        "encrypt_names": false,
        "volumes": { "kind": "single" },
        "content_policy": "cross_platform_clean",
        "excludes": [],
        "output": { "kind": "archive" },
        "destination": {
          "base": "ask",
          "existing_output": "ask"
        },
        "format_options": { "kind": "none" },
        "completion": "none",
        "post_success": "keep_source",
        "test_after_create": false
      }
    }
  ],
  "bindings": {
    "app_default_create": "builtin.create.cross-platform-7z",
    "app_default_extract": "builtin.extract.smart",
    "file_manager_create": "builtin.create.cross-platform-7z",
    "file_manager_extract": "builtin.extract.smart"
  }
}
```

Create presets cover format, compression level, runtime password intent, file
name encryption, exact split size, content policy, exclude rules, standard or
self-extracting output, destination policy, completion behavior, source
retention, creation-time integrity testing and SQZ inner payload. Split sizes
are decimal strings so 64-bit values do not lose precision in JSON. The desktop
app currently accepts values up to JavaScript's exact-integer limit. Runtime
password prompts are valid only for ZIP and 7Z presets, the two create formats
with data encryption.

`content_policy` is required on every create preset. It accepts these
JSON values:

- `cross_platform_clean` adds `.DS_Store`, `._*` and `__MACOSX` to the resolved
  exclude list. It does not exclude other dotfiles.
- `keep_all_files` adds no implicit exclusions.
- `custom` uses only the globs stored in `excludes`.

Only `custom` presets may store rules in `excludes`. The other two policies
require an empty array, which keeps a saved preset unambiguous. Policy
resolution keeps the built-in rule order and removes duplicates. The
`keep_all_files` name refers to selected files; preservation of extended
attributes, resource forks and Finder metadata still depends on the target
format.

Create destinations contain a symbolic `base` and an `existing_output` policy;
they never contain a selected path. The bases are `ask`, `source_parent` and
`default_directory`. The existing-output values are `ask`, `skip`, `overwrite`
and `rename`, shared with extract presets. Create presets accept only
the combinations the desktop shell can execute without ambiguity: `ask` with
`ask`, or either automatic base with `rename`. The desktop shell resolves the
policy to a concrete destination before it submits a job.

`default_directory` means the desktop app's default create folder, not its
extract folder. If that folder is missing or unavailable, the desktop shell
asks for a destination. Automatic destinations choose a conflict-free name and
use a no-replace core commit, so a late collision fails instead of overwriting
another file. An `ask` destination can replace an existing output only after
the user confirms the final path and, for split archives, its output family.
That confirmation carries an opaque core guard bound to the artifact type, the
normalized requested path, and the observed content state covered by the guard:
member names and types, file bytes, selected stable metadata, filesystem
identities, and symlink targets. It does not claim to preserve ACLs, extended
attributes, Finder tags, resource forks, or Windows alternate data streams. If
the final guard check observes a mismatch, the job returns
`destination_changed` before writing a journal or moving an output. An active
path race after that check can instead enter the documented recovery boundary.

`completion` accepts `none`, `reveal_output` and `open_in_squallz`.
`open_in_squallz` is invalid for split or self-extracting output; those outputs
can still be revealed. `post_success` accepts `keep_source` and `trash_source`.
The latter is a request to use the operating system trash only after the output
has completed successfully; it does not authorize permanent deletion.
`test_after_create` requests a complete read of the committed archive using the
same core integrity-test path as the Test command. It is optional for retained
sources and mandatory when `post_success` is `trash_source`; the desktop and
CLI file-manager fallback enforce that test again at job execution time. Source
cleanup is valid only when the resolved exclude list is empty, so Squallz never
moves a selected directory that still contains content omitted from the archive.

The desktop shell deduplicates top-level selections and snapshots their
metadata before creation. After each entry is accepted, the archive writer
reports its source path, archive path, type, link target, size, modification
time, Unix mode and, for a regular file, a BLAKE3 of the bytes it consumed.
After the output is committed, the shell first reopens and tests the full
archive whenever requested or whenever source cleanup is enabled. Only after
that succeeds does it atomically move each top-level source
to a private same-directory holding area and verifies that full manifest there
before using the operating system trash. Before that move, it durably records
the lossless paths and top-level file identity in a private journal and holds a
cross-process lock until the source is trashed, restored or preserved. Startup
recovery is idempotent and refuses corrupt journals or unexpected holding-area
entries. It never trusts a replaced identity: an item Squallz actually moved is
restored or preserved with a no-replace move, while a different item at the
original or preservation path is reported as changed rather than described as
the archived source. If a late file occupies the old path, it is not
overwritten; the verified source is preserved beside it with a
`.squallz-preserved` suffix and the result reports that recovery is required.
Recovery notices retain the actionable path and are visible in both the main
window and standalone task windows. Cleanup may therefore finish as blocked,
partial, failed or cancelled even when the archive itself was created
successfully.

Extract presets cover destination policy, existing-file handling, symbolic
links, filename encoding and runtime password prompting. The current schema accepts the
four destination combinations the desktop app can represent without loss:

- smart layout in the configured default directory;
- an archive-named folder in the configured default directory;
- direct extraction beside the archive;
- a destination chosen when the job starts.

Extract presets currently require `post_success: "keep_source"`. Removing the
source archive is rejected at validation rather than saved as an action the
desktop job cannot execute.

The schema requires three immutable built-ins:

- `builtin.create.cross-platform-7z` uses `cross_platform_clean` and is the
  create default for a newly seeded app and file-manager configuration.
- `builtin.create.balanced-7z` remains available with the `custom` policy and an
  empty exclude list.
- `builtin.extract.smart` remains the default extract preset.

Custom IDs, names, option combinations, exclude globs and bindings are
validated before a revision can be saved.

All create built-ins use `ask` for both destination fields, `none` for
completion, `keep_source` for source retention and disable the optional
creation-time integrity test.

## Passwords and paths

Preset JSON never contains a plaintext password, selected source, final output
path or temporary path. A create preset can say that a password must be asked
for when the job starts. An extract preset can ask only when the archive needs
one. Password Book entries remain in the operating system credential store and
are not referenced from the preset schema.

## Desktop and file-manager behavior

The main create and extract screens apply only an explicitly bound app default.
Clearing a binding is a real unbind; it does not silently select a built-in
preset again.

Finder, Explorer and Linux file-manager actions resolve their binding before a
job is submitted. The resulting `JobSpec` is a complete snapshot, so editing a
preset cannot change a running or queued job. If the GUI handoff is unavailable,
the generated integration scripts call the bundled CLI with
`--file-manager-preset`, which reads the same document.

The file-manager action still owns its explicit destination:

- **Extract Here** enables Smart Extract from the archive's parent directory.
- **Extract to Folder** writes directly to an archive-named folder.
- The bound extract preset contributes conflict, symbolic-link and filename
  encoding policies; it does not replace the action's destination.

File-manager creation is deliberately limited to a standard 7Z archive without
a password prompt. Its bound preset can change compression level, split size,
content policy and creation-time integrity testing. Before submission, the
bound policy is resolved into the final exclude list used by the archive engine.
A newly seeded binding therefore removes `.DS_Store`, `._*` and `__MACOSX`; a
custom binding uses only its saved globs. File-manager bindings cannot request
source cleanup or an in-app open; the explicit action destination remains
authoritative. Incompatible presets cannot be bound. An empty file-manager
binding uses level 5, a single volume and no implicit exclusions rather than
pretending that a named preset is selected.

## Validation

Readers accept only strict version 1 documents. They reject unknown fields,
duplicate JSON fields, unknown enum values, missing required fields and every
other schema version. An invalid document is reported and left untouched; the
user may remove it to seed a fresh current document.

Rust callers that construct `CreatePreset` must provide `content_policy`,
`destination`, `completion` and `test_after_create`. The lower-level format API receives a resolved
`CreateOptions.excludes` list and concrete output path. Direct CLI commands and
batch jobs own their explicit output paths. CLI preset management
edits only reusable policy and bindings; it does not inject paths or plaintext
credentials into the schema. Generated file-manager actions read
the shared preset document.
