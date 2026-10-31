# Owned NPC API — foundation for Civilization-style mods

Status: experimental, implemented in this fork. Owned NPC extension **1**, alongside native hooks API **40**, exposes
authoritative NPC helpers. The server creates independent actors, runs A*
navigation and collision physics, and persists them in the world index. The
integrated client renders separate, interpolated mirrors from bounded snapshots.
No vanilla villager AI, POIs or NPC teleport commands are part of this API.

The implementation is `horizon_mods::owned_npcs`. New code mods in this checkout
use the native mechanism described in [native-mods](../native-mods.md); adding an
interface to the old WebAssembly WIT would not make it available in this build.

`game/owned_npcs.rs` is the authoritative backend, `owned_npc_actor.rs` owns each
independent body/controller, and `game/owned_npc_net.rs` replicates render-only
mirrors through `horizon:owned_npcs`. Neither body nor mirror enters `Game::mobs`.
The native helpers reject calls from the window or an external server mirror;
use them from the integrated server's native tick/lifecycle hook. Native helpers
take the calling mod ID as `owner`, as the existing settings helpers do.

### Native helper names (fork-only extension 1)

`native_npc_ensure`, `native_npc_state`, `native_npc_set_order`,
`native_npc_cancel`, `native_npc_set_profession`, `native_npc_set_appearance`,
and `native_npc_despawn` forward to the registry operations below. Creation is
asynchronous: `pending-spawn` becomes `loaded` on a running server tick. An
unloaded chunk retains its identity and last observed position; it does not
create another entity. The backend recreates that same runtime body when the
chunk loads. `work_seconds` persists elapsed work across chunk/world reloads.

## Operations

| Operation | Contract |
| --- | --- |
| `ensure(owner, request)` | Creates a pending identity once. An existing unloaded identity remains unloaded; repeated calls never reset its position or issue a second spawn. |
| `state(owner, key)` | Reads only the specified owner's stable key, including presence and order revision. |
| `set_order(owner, key, order)` | Validates an idle, movement or timed work order and increments its revision. It does not teleport or pretend that a task completed. |
| `cancel(owner, key)` | Invalidates the old order and pending arrivals with a new revision. |
| `set_appearance(owner, key, appearance)` | Changes validated model and texture resource references independently of profession. Actual texture decoding/UV compatibility remain the renderer's responsibility. |
| `set_profession(owner, key, profession)` | Changes the data-defined role, cancels the previous task, and retains identity, appearance and position. |
| `mark_unloaded(owner, key)` | Retains the world identity after an actor's chunk unloads. |
| `despawn(owner, key)` | Retains a tombstone so an old unloaded actor can be removed when its chunk returns. The same key is not automatically resurrected. |
| `observe(owner, key, revision, position, arrived)` | Native-backend observation. Rejects stale revisions and arrivals outside the requested radius. A failed/finished navigation path is not proof of arrival. |
| `save` / `restore` | Versioned JSON world index, validation of identities, coordinates, resources and per-owner quotas. |

The host must obtain `owner` from its native-mod registration, not from an
untrusted command argument. Native code itself runs with the game's rights;
these ownership checks are application isolation, not a native-code sandbox.

The index is world-owned and must be restored **before** loading actors or
accepting `ensure` calls. Runtime entity numbers are not persistent identities.
Records retain the dimension and last position for unloaded-chunk bookkeeping.
The backend must match actors to `(owner, stable_key)` and honor tombstones.

Model, profession and city are independent data. A profession such as `wood`
does not select vanilla villager behavior. `Work` defines an action identifier
and duration; completion and its effect on city yields belong to the owning
mod's scheduler, not to vanilla farming or an automatic registry timer.

## Validation and limits

- At most 32 indexed identities per owner and 4096 per world, including unloaded
  records and tombstones. Retired keys are deliberately not silently reused.
- Stable keys and owner/city/profession IDs are bounded ASCII identifiers.
- Model, texture, definition and dimension references use `namespace:path`;
  traversal, absolute paths, empty segments and non-finite positions are rejected.
- The registry loads at most 8 MiB and refuses duplicate identities or an unknown
  data version. A host must write atomically and preserve a rejected save.
- Movement arrival radii and work durations are finite and bounded. Invalid
  operations do not partially alter an actor's position or current order.

## Present limitations

- Integrated and external fork LAN clients negotiate the exact protocol version
  `[1]` on `horizon:owned_npc_capabilities`. Only negotiated, loaded clients receive
  snapshots; snapshots grant rendering, never mutation authority. Two real clients
  were tested on 2026-10-31. Dedicated-server deployment remains untested.
- Supported geometry resources are `minecraft:entity/player` and
  `minecraft:entity/villager`, reused only as prepared models. The body has its
  own `owned_npc:<definition>` identity and no vanilla gameplay controller.
- Textures must already exist in the prepared Minecraft atlas. PNG import and
  namespaced skin loading are not implemented. Invalid/unknown appearance
  snapshots are rejected atomically on the client, retaining its last good view.
- Profession is data; `Work` is a timed action, not automatic tree chopping,
  block construction, harvest, or city income. The Civilization mod must drive
  its production/yield changes from the reported task state.
- Independent bodies are not yet vanilla combat/trading/pickup entities. There
  are no custom world interaction events, custom animation definitions or GUI.
- Owned NPC extension 1 is specific to this fork; it is independent of upstream native hooks API 40.

## Real-client development harness

Build this fork (do not replace the official installation):

```sh
cargo build --locked --profile fast -p horizon-client --bin horizon-client --bin horizon-ctl
```

`horizon-ctl owned-npcs JSON` is a trusted local development endpoint that calls
the same native helpers on the integrated server. An envelope looks like:

```json
{"owner":"civilization","command":{"operation":"set-order","key":"citizen_1","order":{"kind":"move-to","target":{"x":10.5,"y":100.0,"z":0.5},"arrival_radius":0.6}}}
```

Other operations are `ensure` (field `npc`: `EnsureNpc`), `state`, `list`,
`cancel`, `set-profession`, `set-appearance`, and `despawn`.
`horizon-ctl owned-npc-mirrors` reports the actual client mirror count. The
control transport is local and token-protected; it is not an NPC network RPC.
The authoritative save is `<world>/horizon-owned-npcs.json`, written atomically.

```sh
python tools/test_owned_npcs_native.py --game target/fast/horizon-client.exe --ctl target/fast/horizon-ctl.exe --resources PATH_TO_PREPARED_26.3 --proof-dir PATH_TO_NEW_PROOFS --leave-running
```

The harness creates a fresh home and uses normal integrated-server single
player. It records `report.json`, the CLI transcript and `owned-npc.png`.
It never uses the deprecated `--in-frame` simulation or the player's saves.
On Windows subprocess output is captured to regular files so a launched game's
inherited pipe handles cannot stall the test until the game exits.

## Current validation

Run `cargo test -p horizon-mods --lib owned_npcs` for tests of persistence across
unload/reload, ownership isolation, stale arrival rejection, bounds/quotas,
tombstones and corrupted/duplicate saves. No Minecraft resources are required.

Real-client checks passed on Windows/Vulkan on 2026-10-30: server creation visible
on the separate client; native walking around a solid wall; timed work; return
to the store; cancel/reassign; save/restart without a duplicate; and despawn
removing the client mirror. Geometry in this run was the prepared villager model
with the independent controller. These results do not imply that the complete
Civilization gameplay mod has been ported to the current native-mod mechanism.

Validation on 2026-10-31: the updated fork passed nine live NPC/save checks, including real A* movement, cancellation, despawn, owner inventory restoration with the campaign disabled, and an intentionally empty inventory staying empty. Civilization passed eight scenarios; two real clients passed six cooperation checks. See [build report](../civilization/BUILD-REPORT.ru.md).
