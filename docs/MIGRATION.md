# Vault migration

## Start a clean Jinx 2.5 vault

Run `./start.sh` or invoke `saeugling init NEW_VAULT` without import arguments.
This creates a new format-3 vault with no learned lexical graph. The start
script creates `private/jinx.db` only when it does not exist; it does not
replace an existing vault.

## Keep an existing version-2 vault

The v2 reader remains available for existing Rust vaults. Opening a version-2
vault does not convert it to format 3; writes to it remain in its original
format. Keep a backup before experimenting with any older model. Its previous
neurons and connections remain; it is not made linguistically empty.

## Explicitly import an old JSON state into a new format-3 vault

```sh
saeugling init NEW_VAULT STATE_JSON ACTIONS_JSON [LEGACY_AUDIT]
```

This creates a distinct new vault. Source JSON and audit files are read but not
modified. The imported graph remains populated and its original neuron IDs are
preserved. Imported action/template metadata is retained as data only: Jinx
does not execute actions or use templates to generate replies. The import is
not a semantic conversion or a training run.

## Archive and source provenance

Only user chat messages and Jinx replies are copied verbatim into the encrypted
format-3 conversation archive. `/train` and `/code` affect graph learning and
keep a content digest/summary, but the source file itself is not copied into
that archive. Keep any file whose exact contents must remain available for
review.
