//! Authoritative, independent NPC actors for native mods. No vanilla mob AI.
use super::{Game, execution_role::ExecutionRole};
use horizon_client::owned_npc_actor::{self, Actor};
use horizon_mods::owned_npcs::{
    Appearance, EnsureNpc, MAX_PER_OWNER, Npc, Order, OrderStatus, Presence, Registry,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

#[derive(Default)]
pub(super) struct OwnedNpcs {
    registry: Registry,
    actors: BTreeMap<(String, String), Actor>,
    path: Option<PathBuf>,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    owner: String,
    command: Command,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
enum Command {
    Ensure { npc: EnsureNpc },
    State { key: String },
    List,
    SetOrder { key: String, order: Order },
    Cancel { key: String },
    SetProfession { key: String, profession: String },
    SetAppearance { key: String, appearance: Appearance },
    Despawn { key: String },
}

impl OwnedNpcs {
    fn save(&self) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        if self.registry.records().next().is_none() && !path.exists() {
            return Ok(());
        }
        let bytes = self.registry.save()?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    }
}

impl Game<'_> {
    fn bind_owned_npcs(&mut self) -> Result<(), String> {
        if self.options.execution_role == ExecutionRole::Client || self.remote.is_some() {
            return Err("Owned NPC changes require the authoritative server thread".into());
        }
        if self.world_root.as_os_str().is_empty() {
            return Err("No world is open".into());
        }
        let path = self.world_root.join("horizon-owned-npcs.json");
        if self.owned_npcs.path.as_ref() != Some(&path) {
            self.owned_npcs.save()?;
            let registry = match fs::metadata(&path) {
                Ok(meta) if meta.len() > 8 * 1024 * 1024 => {
                    Err("Owned NPC index exceeds 8 MiB".into())
                }
                Ok(_) => fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| Registry::restore(&bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
                Err(e) => Err(e.to_string()),
            };
            match registry {
                Ok(registry) => {
                    self.owned_npcs = OwnedNpcs {
                        registry,
                        path: Some(path),
                        ..Default::default()
                    }
                }
                Err(error) => {
                    self.owned_npcs = OwnedNpcs {
                        path: Some(path),
                        error: Some(error.clone()),
                        ..Default::default()
                    };
                    return Err(error);
                }
            }
        }
        if let Some(error) = &self.owned_npcs.error {
            return Err(error.clone());
        }
        Ok(())
    }

    pub(super) fn prepare_owned_npcs(&mut self) {
        let _ = self.bind_owned_npcs();
    }

    /// Reserve no state, but reject permanent storage/quota failures before a
    /// campaign accepts its founding payment. Unloaded and despawned identities
    /// still occupy their registry slots.
    pub(super) fn preflight_owned_npc_keys(
        &mut self,
        owner: &str,
        keys: &[String],
    ) -> Result<(), String> {
        self.bind_owned_npcs()?;
        let records: Vec<_> = self.owned_npcs.registry.records().collect();
        let new_keys: std::collections::BTreeSet<_> = keys
            .iter()
            .filter(|key| {
                !records
                    .iter()
                    .any(|npc| npc.owner == owner && npc.stable_key == key.as_str())
            })
            .collect();
        let owned = records.iter().filter(|npc| npc.owner == owner).count();
        if owned + new_keys.len() > MAX_PER_OWNER || records.len() + new_keys.len() > 4096 {
            return Err(
                "Нет свободных мест для представителей новой общины: достигнут лимит NPC".into(),
            );
        }
        Ok(())
    }

    pub(super) fn owned_npc_template(
        &self,
        appearance: &Appearance,
    ) -> Result<Arc<horizon_client::mobs::MobType>, String> {
        let template = owned_npc_actor::geometry_template(appearance)?;
        owned_npc_actor::texture_key(appearance)?;
        self.res
            .mob_types()
            .get(template)
            .cloned()
            .ok_or_else(|| format!("Missing owned NPC geometry {template}"))
    }

    pub(super) fn ensure_owned_npc(
        &mut self,
        owner: &str,
        request: EnsureNpc,
    ) -> Result<Npc, String> {
        self.bind_owned_npcs()?;
        self.owned_npc_template(&request.appearance)?;
        if request.dimension != self.dim.id() {
            return Err("Create owned NPCs in the current server dimension".into());
        }
        let result = self.owned_npcs.registry.ensure(owner, request)?.clone();
        self.owned_npcs.save()?;
        Ok(result)
    }

    pub(super) fn owned_npc_state(&mut self, owner: &str, key: &str) -> Result<Npc, String> {
        self.bind_owned_npcs()?;
        self.owned_npcs.registry.state(owner, key).cloned()
    }

    pub(super) fn order_owned_npc(
        &mut self,
        owner: &str,
        key: &str,
        order: Order,
    ) -> Result<u64, String> {
        self.bind_owned_npcs()?;
        let revision = self.owned_npcs.registry.set_order(owner, key, order)?;
        self.owned_npcs.save()?;
        Ok(revision)
    }

    pub(super) fn cancel_owned_npc(&mut self, owner: &str, key: &str) -> Result<u64, String> {
        self.bind_owned_npcs()?;
        let revision = self.owned_npcs.registry.cancel(owner, key)?;
        self.owned_npcs.save()?;
        Ok(revision)
    }

    pub(super) fn profession_owned_npc(
        &mut self,
        owner: &str,
        key: &str,
        profession: &str,
    ) -> Result<u64, String> {
        self.bind_owned_npcs()?;
        let revision = self
            .owned_npcs
            .registry
            .set_profession(owner, key, profession)?;
        self.owned_npcs.save()?;
        Ok(revision)
    }

    pub(super) fn appearance_owned_npc(
        &mut self,
        owner: &str,
        key: &str,
        appearance: Appearance,
    ) -> Result<(), String> {
        self.bind_owned_npcs()?;
        self.owned_npc_template(&appearance)?;
        self.owned_npcs
            .registry
            .set_appearance(owner, key, appearance)?;
        self.owned_npcs.save()
    }

    pub(super) fn despawn_owned_npc(&mut self, owner: &str, key: &str) -> Result<(), String> {
        self.bind_owned_npcs()?;
        self.owned_npcs.registry.despawn(owner, key)?;
        self.owned_npcs.actors.remove(&(owner.into(), key.into()));
        self.owned_npcs.save()
    }

    pub(super) fn tick_owned_npcs(&mut self) {
        if self.options.execution_role == ExecutionRole::Client
            || self.owned_npcs.path.is_none()
            || self.owned_npcs.error.is_some()
            || !self.clock.world_runs()
        {
            return;
        }
        if self.owned_npcs.registry.records().next().is_none() {
            return;
        }
        let mut state = std::mem::take(&mut self.owned_npcs);
        let records = state
            .registry
            .records()
            .filter(|n| n.dimension == self.dim.id())
            .cloned()
            .collect::<Vec<_>>();
        for npc in records {
            let key = (npc.owner.clone(), npc.stable_key.clone());
            if npc.presence == Presence::Despawned {
                state.actors.remove(&key);
                continue;
            }
            if !self
                .world
                .loaded(npc.position.x.floor() as i32, npc.position.z.floor() as i32)
            {
                let _ = state.registry.mark_unloaded(&npc.owner, &npc.stable_key);
                state.actors.remove(&key);
                continue;
            }
            let template = match self.owned_npc_template(&npc.appearance) {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("Owned NPC geometry: {e}");
                    continue;
                }
            };
            if !state.actors.contains_key(&key) {
                match Actor::new(&npc, template.clone()) {
                    Ok(actor) => {
                        state.actors.insert(key.clone(), actor);
                    }
                    Err(e) => {
                        log::warn!("Owned NPC body: {e}");
                        continue;
                    }
                }
            }
            let actor = state.actors.get_mut(&key).expect("created actor");
            if actor.appearance != npc.appearance && actor.set_geometry(&npc, template).is_err() {
                continue;
            }
            let observation = {
                let world = self.mob_world_base(&[]);
                actor.tick(&world, &npc)
            };
            if !observation.loaded {
                let _ = state.registry.mark_unloaded(&npc.owner, &npc.stable_key);
                continue;
            }
            let status = if matches!(
                npc.order_status,
                OrderStatus::Complete | OrderStatus::Cancelled
            ) {
                npc.order_status
            } else if observation.arrived
                || observation.work_finished
                || matches!(npc.order, Order::Idle)
            {
                OrderStatus::Complete
            } else if observation.blocked {
                OrderStatus::Blocked
            } else {
                OrderStatus::Running
            };
            if let Err(error) = state.registry.update_runtime(
                &npc.owner,
                &npc.stable_key,
                observation.revision,
                observation.position,
                status,
                actor.work_seconds,
            ) {
                log::warn!("Owned NPC observation: {error}");
            }
        }
        if self.ticks.is_multiple_of(100)
            && let Err(error) = state.save()
        {
            log::warn!("Owned NPC save: {error}");
        }
        self.owned_npcs = state;
    }

    pub(super) fn save_owned_npcs(&self) -> Result<(), String> {
        self.owned_npcs.save()
    }

    pub(super) fn has_owned_npc_backend(&self) -> bool {
        self.owned_npcs.registry.records().next().is_some()
    }

    pub(super) fn owned_npc_wire(&self) -> Vec<super::owned_npc_net::WireNpc> {
        self.owned_npcs
            .actors
            .iter()
            .filter_map(|((owner, key), actor)| {
                let npc = self.owned_npcs.registry.state(owner, key).ok()?;
                (npc.dimension == self.dim.id() && npc.presence == Presence::Loaded).then(|| {
                    super::owned_npc_net::WireNpc {
                        npc: npc.clone(),
                        yaw: actor.body.yaw,
                        body_yaw: actor.body.body_yaw,
                        head_yaw: actor.body.head_yaw,
                        head_pitch: actor.body.head_pitch,
                    }
                })
            })
            .take(128)
            .collect()
    }

    pub(super) fn owned_npc_control(&mut self, request: Request) -> Value {
        let owner = request.owner;
        let result = (|| -> Result<Value, String> {
            match request.command {
                Command::Ensure { npc } => {
                    Ok(json!({"ok":true,"npc":self.native_npc_ensure(&owner,npc)?}))
                }
                Command::State { key } => {
                    Ok(json!({"ok":true,"npc":self.native_npc_state(&owner,&key)?}))
                }
                Command::List => {
                    self.bind_owned_npcs()?;
                    Ok(
                        json!({"ok":true,"authority":"server","dimension":self.dim.id(),"npcs":self.owned_npcs.registry.records().filter(|n|n.owner==owner).collect::<Vec<_>>(),"distance_walked":self.owned_npcs.actors.iter().filter(|((o,_),_)|o==&owner).map(|((_,key),a)|(key,a.distance_walked)).collect::<BTreeMap<_,_>>()}),
                    )
                }
                Command::SetOrder { key, order } => {
                    Ok(json!({"ok":true,"revision":self.native_npc_set_order(&owner,&key,order)?}))
                }
                Command::Cancel { key } => {
                    Ok(json!({"ok":true,"revision":self.native_npc_cancel(&owner,&key)?}))
                }
                Command::SetProfession { key, profession } => Ok(
                    json!({"ok":true,"revision":self.native_npc_set_profession(&owner,&key,&profession)?}),
                ),
                Command::SetAppearance { key, appearance } => {
                    self.native_npc_set_appearance(&owner, &key, appearance)?;
                    Ok(json!({"ok":true}))
                }
                Command::Despawn { key } => {
                    self.native_npc_despawn(&owner, &key)?;
                    Ok(json!({"ok":true}))
                }
            }
        })();
        result.unwrap_or_else(|error| json!({"error":error}))
    }
}
