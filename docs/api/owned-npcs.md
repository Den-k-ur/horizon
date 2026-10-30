# Owned NPC API — foundation for Civilization-style mods

Status: experimental Rust API foundation. This patch does **not** spawn or render
entities, move an actor, add a network entity type, or expose a new native hook.
It supplies the persistent registry and command semantics that an authoritative
native backend will use. No vanilla villager AI, POIs or teleport commands are
part of this API.

The implementation is `horizon_mods::owned_npcs`. New code mods in this checkout
use the native mechanism described in [native-mods](../native-mods.md); adding an
interface to the old WebAssembly WIT would not make it available in this build.

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

## Next integration patches

1. Add an authoritative native NPC backend with a distinct gameplay type,
   controller and persistent world index; reuse low-level navigation and physics.
2. Connect creation, movement observation, cancellation and tombstones to chunk
   lifecycle. Keep unloaded identities out of a second spawn path.
3. Register matching definition/entity IDs on the integrated server and client,
   and add rendering plus explicitly validated referenced skin assets.
4. Expose native-mod helper methods/hooks around these operations and version the
   native contract. The host binds the registered owner automatically.
5. Exercise ensure → move → arrive/work → cancel/reassign → unload/reload in a
   real client. The unit tests below do not constitute that gameplay proof.

## Current validation

Run `cargo test -p horizon-mods --lib owned_npcs` for tests of persistence across
unload/reload, ownership isolation, stale arrival rejection, bounds/quotas,
tombstones and corrupted/duplicate saves. No Minecraft resources are required.
