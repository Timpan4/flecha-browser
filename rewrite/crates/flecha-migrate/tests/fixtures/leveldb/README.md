# C++ LevelDB fixtures

These synthetic databases were generated with `classic-level` 3.0.0 (C++ LevelDB 1.20 with Snappy), using the independent verifier's fixture writer. Keys and values use UTF-8. The `state` and `state:last-good` values are extension storage JSON strings containing serialized v2 snapshots. The only tab in each snapshot has an `example.test` URL.

| Fixture | Writer operations |
| --- | --- |
| `logonly` | Put current `state`, close. |
| `snappy` | Put current `state` and backup, compact the full key range, reopen, close. |
| `twolevel` | Put old `state` and backup, compact the full key range, put new `state`, reopen twice, close. |

The fixtures include only the manifest, journal, table and `CURRENT` files. The integration tests verify the intact copies, then corrupt a byte of the WAL or Snappy data block, or truncate a file, in a separate temporary copy. [ClassicLevel source](https://github.com/Level/classic-level/tree/v3.0.0)
