//! A melee hit on the single-player zombie: `IsoZombie.Hit`, `IsoGameCharacter.Hit`/`processHitDamage`/
//! `hitConsequences`, `IsoZombie.hitConsequences`, `CombatManager.processHit` and the states a hit leads to.
//!
//! The wielder is the player stand-in of the headless world. Its values the chain reads were measured through the
//! oracle (`tests/original_probe/ZombieAiProbe.java`, `PZ_HITINFO`): no traits, full endurance, `knockbackAttackMod`
//! 1, weapon level 0, `Strength` level 0 (`getShovingMod` 0.75), not shoving, not aiming at the floor, facing west
//! `(-1, 0)`, both hands empty (`isItemInBothHands` false), `ZombieHitReaction` unset.
//!
//! NOT PORTED (UNKNOWN, each an `Err`): ranged and explosive weapons (the ballistics path), a shove, traits, perk
//! levels, a Sharpness other than the scripts' 1.0, the `Kill`/`die` path until the corpse is ported, the wielder's
//! XP (`AddXP` of the knock-back bonus is invisible to the zombie), the `OnHitZombie` Lua handler's texture damage
//! (visual only; it draws no random number in any scenario measured), the wall blood splats a hit leaves
//! (`IsoWallBloodSplat` objects: their random draws are reproduced, the objects are not kept).
use super::iso_zombie::{IsoZombie, ZombieState, ZombieWorld};
use super::iso_zombie_sp::{SpRead, Sq};
use crate::zombie::core::math::pz_math;
use crate::zombie::iso::iso_directions::IsoDirections;
use crate::zombie::iso::iso_grid_square::IsoGridSquare;
use crate::zombie::iso::sprite_details::iso_flag_type::IsoFlagType;
use crate::zombie::iso::vector2::Vector2;
use crate::zombie::scripting::item::{Item, Value};
use crate::zombie::scripting::objects::resource_location::ResourceLocation;

/// `CombatConfigKey` defaults (`CombatConfig` of a game that never changed them).
const GLOBAL_MELEE_DAMAGE_REDUCTION_MULTIPLIER: f32 = 0.15;
const BASE_WEAPON_DAMAGE_MULTIPLIER: f32 = 0.3;
const WEAPON_LEVEL_DAMAGE_MULTIPLIER_INCREMENT: f32 = 0.1;
const NON_PLAYER_RECEIVED_DAMAGE_MULTIPLIER: f32 = 1.5;
const DAMAGE_PENALTY_ONE_HANDED_TWO_HANDED_WEAPON_MULTIPLIER: f32 = 0.5;

/// What the player stand-in answers the hit chain with (measured, see the module comment).
const WIELDER_SHOVING_MOD: f32 = 0.75;
const WIELDER_ENDURANCE: f32 = 1.0;
const WIELDER_KNOCKBACK_ATTACK_MOD: f32 = 1.0;
const WIELDER_WEAPON_LEVEL: i32 = 0;
const WIELDER_FORWARD: (f32, f32) = (-1.0, 0.0);

/// `BodyPartType` ordinals the hit reads.
const BODY_PART_TORSO_UPPER: i32 = 6;
const BODY_PART_TORSO_LOWER: i32 = 7;
/// `BloodBodyPartType` ordinals `applyBlood` draws between.
const BLOOD_UPPER_ARM_L: i32 = 4;
const BLOOD_GROIN: i32 = 10;

/// zombie.inventory.types.HandWeapon, the fields the hit chain reads, made from the item script
/// (`Item.InstanceItem`'s weapon branch, `HandWeapon.setScriptItem`).
#[derive(Clone, Debug)]
pub struct HitWeapon {
    /// `moduleDotType`
    pub full_name: String,
    /// `weaponCategories` (`Categories = base:blunt;...`)
    pub categories: Vec<ResourceLocation>,
    /// `isMelee`: `!weaponCategories.isEmpty()`
    pub melee: bool,
    /// `isExplosive`: `"Explosives".equals(displayCategory)`
    pub explosive: bool,
    pub ranged: bool,
    pub push_back_mod: f32,
    pub knock_back_on_no_death: bool,
    pub splat_blood_on_no_death: bool,
    /// `getSplatNumber()`: the script's `SplatNumber` (default 2), the blood splats of a kill
    pub splat_number: i32,
    /// `getCriticalDamageMultiplier()`: the script's `critDmgMultiplier` times the Sharpness multiplier
    pub critical_damage_multiplier: f32,
    /// `hitAngleMod`, radians
    pub hit_angle_mod: f32,
    pub two_hand_weapon: bool,
    /// The size of the `WeaponSpritesByIndex` list, else the larger of the `StaticModelsByIndex` and
    /// `WorldStaticModelsByIndex` lists (`InventoryItem.synchWithVisual` picks `modelIndex = Rand.Next(size)`).
    pub model_index_count: Option<i32>,
}

impl HitWeapon {
    /// Item.InstanceItem (weapon branch) for the hit chain's fields. `Err` for what the port does not model.
    pub fn from_item(item: &Item) -> Result<HitWeapon, String> {
        let f = |name: &str| match item.get(name) {
            Some(Value::F32(v)) => Ok(*v),
            _ => Err(format!("Item field {name} is not a float")),
        };
        let b = |name: &str| match item.get(name) {
            Some(Value::Bool(v)) => Ok(*v),
            _ => Err(format!("Item field {name} is not a boolean")),
        };
        let mut categories = Vec::new();
        let mut sharpness: Option<f32> = None;
        let mut weapon_sprites: Option<i32> = None;
        let mut static_models: Option<i32> = None;
        let mut world_static_models: Option<i32> = None;
        let count = |val: &str| crate::java::split_char(val, ';').len() as i32;
        for (key, val) in &item.unported {
            match key.as_str() {
                "categories" => {
                    for part in val.split(';') {
                        categories.push(ResourceLocation::of(part.trim())?);
                    }
                }
                "weaponspritesbyindex" => weapon_sprites = Some(count(val)),
                "staticmodelsbyindex" => static_models = Some(count(val)),
                "worldstaticmodelsbyindex" => world_static_models = Some(count(val)),
                "sharpness" => {
                    sharpness = Some(
                        crate::java::parse_float(val)
                            .ok_or_else(|| format!("{}: Sharpness {val}", item.full_name()))?,
                    )
                }
                _ => {}
            }
        }
        let explosive =
            matches!(item.get("displayCategory"), Some(Value::Str(Some(s))) if s == "Explosives");
        let ranged = b("ranged")?;
        if ranged || explosive {
            return Err(format!(
                "{}: a ranged or explosive weapon's hit (ballistics) is not ported",
                item.full_name()
            ));
        }
        // `hasSharpness()`: the script's Sharpness attribute; its multiplier is `(value + 1) / 2`, measured for the
        // scripts' 1.0 only
        let mut critical_damage_multiplier = f("critDmgMultiplier")?;
        if let Some(s) = sharpness {
            if s != 1.0 {
                return Err(format!(
                    "{}: a Sharpness of {s} is not measured (UNKNOWN)",
                    item.full_name()
                ));
            }
            critical_damage_multiplier *= (s + 1.0f32) / 2.0f32;
        }
        // Item.InstanceItem: the static model lists are kept only when non-empty (a list is never empty here)
        let model_index_count = if weapon_sprites.is_some() {
            weapon_sprites
        } else {
            match (static_models, world_static_models) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                (None, None) => None,
            }
        };
        Ok(HitWeapon {
            full_name: item.full_name(),
            melee: !categories.is_empty(),
            categories,
            explosive,
            ranged,
            push_back_mod: f("pushBackMod")?,
            knock_back_on_no_death: b("knockBackOnNoDeath")?,
            splat_blood_on_no_death: b("splatBloodOnNoDeath")?,
            splat_number: match item.get("splatNumber") {
                Some(Value::I32(v)) => *v,
                _ => return Err("Item field splatNumber is not an integer".to_string()),
            },
            critical_damage_multiplier,
            // `(float) Math.toRadians(hitAngleMod)`
            hit_angle_mod: f64::from(f("hitAngleMod")?).to_radians() as f32,
            two_hand_weapon: b("twoHandWeapon")?,
            model_index_count,
        })
    }

    /// InventoryItemFactory.CreateItem(type) -> Item.InstanceItem: the random draws of making the item. The outfit
    /// seed `OutfitRNG.setSeed(Rand.Next(Integer.MAX_VALUE))`, then `synchWithVisual`'s `modelIndex = Rand.Next(size)`
    /// when the script lists weapon sprites or static models by index, then `item.id = Rand.Next(2146250223)`. (The
    /// item configurator and `initialiseItem` draw nothing for the weapons measured.)
    pub fn create_item_draws(&self, world: &mut ZombieWorld) {
        let _ = world.rand.next(i32::MAX);
        if let Some(n) = self.model_index_count {
            let _ = world.rand.next(n);
        }
        let _ = world.rand.next(2146250223);
    }

    /// HandWeapon.isOfWeaponCategory(base:<path>)
    pub fn is_of_category(&self, path: &str) -> bool {
        self.categories
            .iter()
            .any(|c| c.namespace == "base" && c.path == path)
    }

    /// HandWeapon.getStaggerBackTimeMod(wielder, target) for a zombie target and the stand-in wielder:
    /// `pushBackMod * getKnockbackMod(wielder) * wielder.getShovingMod()`, 0 for a small blade.
    pub fn stagger_back_time_mod(&self) -> f32 {
        if self.is_of_category("smallblade") {
            0.0
        } else {
            // getKnockbackMod: 2.0 for an axe at Axe level 6 or more (the stand-in has level 0)
            self.push_back_mod * 1.0f32 * WIELDER_SHOVING_MOD
        }
    }
}

/// IsoMovingObject.getVectorFromDirection(vec, dir)
fn vector_from_direction(dir: IsoDirections) -> Vector2 {
    let (x, y) = match dir {
        IsoDirections::S => (0.0, 1.0),
        IsoDirections::N => (0.0, -1.0),
        IsoDirections::E => (1.0, 0.0),
        IsoDirections::W => (-1.0, 0.0),
        IsoDirections::NW => (-1.0, -1.0),
        IsoDirections::NE => (1.0, -1.0),
        IsoDirections::SW => (-1.0, 1.0),
        IsoDirections::SE => (1.0, 1.0),
    };
    let mut v = Vector2::new(x, y);
    v.normalize();
    v
}

/// IsoUtils.DistanceToSquared(fromX, fromY, toX, toY): `(float) (pow(dx, 2.0) + pow(dy, 2.0))`
fn distance_to_squared(from_x: f32, from_y: f32, to_x: f32, to_y: f32) -> f32 {
    let dx = f64::from(to_x - from_x);
    let dy = f64::from(to_y - from_y);
    (dx * dx + dy * dy) as f32
}

/// `IsoGridSquare.testCollideAdjacent(null, x, y, z)`: no mover, so no special object is asked; the stored collide
/// matrix (`useSlowCollision` false) and the `-1` marker decide.
fn test_collide_adjacent_no_mover(sq: &IsoGridSquare, x: i32, y: i32, z: i32) -> bool {
    if sq.collide_matrix == -1 {
        return true;
    }
    if !((-1..=1).contains(&x) && (-1..=1).contains(&y) && (-1..=1).contains(&z)) {
        return true;
    }
    sq.collide_matrix & (1 << ((x + 1) + (y + 1) * 3 + (z + 1) * 9)) != 0
}

impl IsoZombie {
    /// zombie.characters.IsoZombie.Hit(weapon, wielder, damageSplit, bIgnoreDamage, modDelta, bRemote) for a melee
    /// hit by the player stand-in. Draws as the original: the body part, `NextBool(7)`, the crit head, then
    /// `IsoGameCharacter.Hit` (see `hit_character`), then the stagger back when the zombie is close and attacking.
    pub fn hit(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
        damage_split: f32,
        modifiers: HitModifiers,
    ) -> Result<f32, String> {
        let mut body_part = world
            .rand
            .next_range(BODY_PART_TORSO_UPPER, BODY_PART_TORSO_LOWER + 1);
        if world.rand.next_bool(7) {
            body_part = BODY_PART_HEAD;
        }
        if rd.world.player.critical_hit && world.rand.next_bool(3) {
            body_part = BODY_PART_HEAD;
        }
        // LuaEventManager.triggerEvent("OnHitZombie", ...): DamageModelDefinitions.OnHitZombie (visual damage
        // textures; no draw measured)
        self.sp.last_hit_part = body_part;
        let damage = self.hit_character(world, rd, weapon, damage_split, modifiers)?;
        self.time_since_seen_flesh = 0.0;
        let dist_to_proper = {
            let m = &self.character.moving;
            let p = &rd.world.player;
            let sq = distance_to_squared(m.x, m.y, p.x, p.y);
            f64::from(sq).sqrt() as f32
        };
        if !self.is_dead()
            && !self.on_floor
            && !modifiers.ignore_damage
            && dist_to_proper <= 0.9
            && matches!(
                self.current_state,
                Some(ZombieState::Attack) | Some(ZombieState::Lunge)
            )
        {
            self.sp.hit_force = 0.5;
            self.change_state(ZombieState::StaggerBack, world)?;
        }
        Ok(damage)
    }

    /// `IsoZombie.Hit` with the single-player world's read view made as `tick_sp` makes it.
    pub fn hit_sp(
        &mut self,
        world: &mut ZombieWorld,
        squares: &dyn crate::zombie::iso::iso_grid_square::GetSquare,
        weapon: &HitWeapon,
        damage_split: f32,
    ) -> Result<f32, String> {
        let spw = world
            .sp
            .clone()
            .ok_or("IsoZombie.Hit: the world has no single-player part")?;
        let rd = SpRead::new(squares, &spw);
        let damage = self.hit(world, &rd, weapon, damage_split, HitModifiers::default())?;
        rd.check()?;
        Ok(damage)
    }

    /// IsoGameCharacter.isDead: `health <= 0`
    pub fn is_dead(&self) -> bool {
        self.character.health <= 0.0
    }

    /// zombie.characters.IsoGameCharacter.Hit(weapon, wielder, damageSplit, bIgnoreDamage, modDelta, bRemote) for a
    /// zombie target.
    fn hit_character(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
        mut damage_split: f32,
        modifiers: HitModifiers,
    ) -> Result<f32, String> {
        if weapon.melee && !modifiers.ignore_damage {
            self.sp.hit_time += 1;
            if self.sp.hit_time >= 4 && !modifiers.remote {
                damage_split *= (self.sp.hit_time as f32 - 2.0f32) * 1.5f32;
            }
        }
        // `player.isDoShove() && !wielder.isAimAtFloor()`: the stand-in does not shove
        // OnWeaponHitCharacter, OnPlayerGetDamage, LuaHookManager.TriggerHook("WeaponHitCharacter"): no handler
        // changes anything (no random draw measured)
        self.sp.stagger_time_mod = weapon.stagger_back_time_mod();
        // wielder.addWorldSoundUnlessInvisible(5, 1, false): a world sound nobody answers within a scenario (the
        // zombie reacts to sounds only after `timeSinceSeenFlesh > 240`, which a hit resets)
        self.calculate_hit_direction(weapon, rd);
        // setAttackedBy(wielder): the stand-in
        let damage = if modifiers.remote {
            damage_split
        } else {
            self.process_hit_damage(weapon, rd, damage_split, modifiers)
        };
        self.hit_consequences(world, rd, weapon, modifiers, damage)?;
        Ok(damage)
    }

    /// IsoGameCharacter.calculateHitDirection: from the wielder through the zombie, scaled by the push back and
    /// turned by `hitAngleMod`.
    fn calculate_hit_direction(&mut self, weapon: &HitWeapon, rd: &SpRead) {
        let m = &self.character.moving;
        let p = &rd.world.player;
        let mut dir = Vector2::new(m.x, m.y);
        // `handWeapon.getAttackTargetSquare(null)` is null for a hand weapon that is not a trap
        dir.x -= p.x;
        dir.y -= p.y;
        dir.normalize();
        dir.x *= weapon.push_back_mod;
        dir.y *= weapon.push_back_mod;
        dir.rotate(weapon.hit_angle_mod);
        self.sp.hit_dir = dir;
    }

    /// IsoGameCharacter.processHitDamage: the damage and (as a side effect) the hit force.
    fn process_hit_damage(
        &mut self,
        weapon: &HitWeapon,
        rd: &SpRead,
        damage_split: f32,
        modifiers: HitModifiers,
    ) -> f32 {
        let p = &rd.world.player;
        // weapon.isExplosive() is refused by HitWeapon::from_item
        let mut damage = damage_split * modifiers.mod_delta;
        let mut dmg_test = damage;
        if modifiers.ignore_damage {
            dmg_test = damage / 2.7f32;
        }
        let mut f = dmg_test * WIELDER_SHOVING_MOD;
        if f > 1.0 {
            f = 1.0;
        }
        self.sp.hit_force = f;
        // STRONG / WEAK traits: the stand-in has none
        let mut del = WIELDER_ENDURANCE;
        del *= WIELDER_KNOCKBACK_ATTACK_MOD;
        if del < 0.5 {
            del *= 1.3f32;
            if del < 0.4 {
                del = 0.4;
            }
            self.sp.hit_force *= del;
        }
        // `wielder instanceof IsoPlayer && !bIgnoreDamage`
        if !modifiers.ignore_damage {
            self.sp.hit_force *= 2.0f32;
        }
        // `!isoPlayer.isDoShove() && !bIgnoreDamage`
        if !modifiers.ignore_damage {
            let m = &self.character.moving;
            let mut o_pos = Vector2::new(m.x, m.y);
            o_pos.x -= p.x;
            o_pos.y -= p.y;
            let dir = vector_from_direction(self.character.forward_iso_direction());
            o_pos.normalize();
            let dot = o_pos.x * dir.x + o_pos.y * dir.y;
            if dot > -0.3f32 {
                damage *= 1.5f32;
            }
        }
        // CombatManager.applyPlayerReceivedDamageModifier: the zombie is no IsoPlayer
        damage *= NON_PLAYER_RECEIVED_DAMAGE_MULTIPLIER;
        // CombatManager.applyWeaponLevelDamageModifier
        let weapon_level = WIELDER_WEAPON_LEVEL.clamp(0, 10);
        damage *= BASE_WEAPON_DAMAGE_MULTIPLIER
            + weapon_level as f32 * WEAPON_LEVEL_DAMAGE_MULTIPLIER_INCREMENT;
        // `isAimAtFloor()` is false for the stand-in
        if p.critical_hit && !modifiers.ignore_damage {
            damage *= f32::max(2.0f32, weapon.critical_damage_multiplier);
        }
        // CombatManager.applyOneHandedDamagePenalty: `isItemInBothHands(weapon)` is false (empty hands)
        let using_two_handed_incorrectly = weapon.two_hand_weapon;
        damage
            * if using_two_handed_incorrectly {
                DAMAGE_PENALTY_ONE_HANDED_TWO_HANDED_WEAPON_MULTIPLIER
            } else {
                1.0
            }
    }

    /// IsoGameCharacter.hitConsequences, then IsoZombie.hitConsequences (the zombie's override calls super first).
    fn hit_consequences(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
        modifiers: HitModifiers,
        mut damage: f32,
    ) -> Result<(), String> {
        // IsoZombie.hitConsequences: `!isOnlyJawStab() || isCloseKilled()` (the jaw stab flag is never set here)
        if !modifiers.ignore_damage {
            // applyGlobalDamageReductionMultipliers
            if weapon.melee {
                damage *= GLOBAL_MELEE_DAMAGE_REDUCTION_MULTIPLIER;
            }
            // CombatManager.applyDamage -> `isInvulnerable()` is false -> IsoGameCharacter.applyDamage
            self.character.health -= damage;
            if self.character.health < 0.0 {
                self.character.health = 0.0;
            }
        }
        if self.is_dead() {
            // `!isOnKillDone()`: wielder.setZombieKills(+1), then `Kill(weapon, wielder)` (no server, no client)
            if !self.sp.on_kill_done {
                self.kill(world, rd, weapon)?;
            }
        } else {
            if weapon.splat_blood_on_no_death {
                self.splat_blood(world, rd, 2, 0.2f32)?;
            }
            // `!weapon.isRanged() && knockBackOnNoDeath`: wielder.xp.AddXP(Strength, 2.0F), invisible to the zombie
        }
        // IsoZombie.hitConsequences
        let anim = self
            .anim
            .as_mut()
            .ok_or("IsoZombie.hitConsequences: no animator")?;
        anim.action_context.report_event("wasHit");
        if !modifiers.remote {
            self.process_hit(world, weapon, rd)?;
        }
        // setTarget(wielder): `GameClient.client` is false
        self.sp.target = true;
        // `!GameServer.server && !GameClient.client`: setKnockedDown(crit || onFloor || alwaysKnockedDown), the zombie is never always knocked down
        self.sp.knocked_down = rd.world.player.critical_hit || self.on_floor;
        // checkClimbOverFenceHit / checkClimbThroughWindowHit: the zombie is in neither state
        if self.should_become_crawler(world, rd) {
            self.sp.become_crawler = true;
        }
        Ok(())
    }

    /// IsoZombie.shouldBecomeCrawler(attacker): the player stand-in, neither shoving nor aiming at the floor.
    fn should_become_crawler(&mut self, world: &mut ZombieWorld, _rd: &SpRead) -> bool {
        if self.sp.become_crawler {
            return true;
        }
        if self.crawling {
            return false;
        }
        // Core.lastStand is false
        if self.is_dead() {
            return false;
        }
        // isCloseKilled() is false; `!player.isAimAtFloor() && player.isDoShove()` is false
        world.rand.next_bool(30)
    }

    /// zombie.CombatManager.processHit(weapon, wielder, target) for a melee weapon: the draws of
    /// `setUsePhysicHitReaction`, `calculateShotDirection` and `applyBlood`, then the hit reaction.
    fn process_hit(
        &mut self,
        world: &mut ZombieWorld,
        weapon: &HitWeapon,
        rd: &SpRead,
    ) -> Result<(), String> {
        // HitReaction.fromString(wielder.getVariableString("ZombieHitReaction")): unset -> NONE
        // setUsePhysicHitReaction: `canRagdoll()` (the zombie has a state and its animation player), then
        // RagdollSettingsManager.usePhysicHitReaction: `Rand.Next(0.0F, 100.0F)`; only a ranged weapon uses it
        let can_ragdoll = self.sp.anim_player.is_some() && self.current_state.is_some();
        if !can_ragdoll {
            self.sp.use_physic_hit_reaction = false;
        } else {
            let _chance = world.rand.next_float(0.0, 100.0);
            // only a ranged weapon uses the result (a ranged weapon is an `Err` in `HitWeapon::from_item`)
            self.sp.use_physic_hit_reaction = false;
        }
        // calculateShotDirection(wielder, target)
        self.calculate_shot_direction(world);
        // applyBlood with HitReaction.NONE: the default branch
        // (a blunt weapon and any weapon that is not unarmed draw the same random body part)
        if weapon.is_of_category("blunt") || !weapon.is_of_category("unarmed") {
            let _part = world.rand.next_range(BLOOD_UPPER_ARM_L, BLOOD_GROIN);
        }
        // `target instanceof IsoZombie && getEatBodyTarget() != null`: the zombie eats nothing here
        // `hitReaction == FLOOR`: no
        // hitReaction == NONE: setStaggerBack(true), setHitReaction("")
        let _ = rd;
        self.sp.stagger_back = true;
        self.sp.hit_reaction = String::new();
        // createCombatData: `usePhysicHitReaction()` is false
        Ok(())
    }

    /// CombatManager.calculateShotDirection(wielder, target): the angle between the wielder's and the zombie's
    /// forward directions decides which draw is made.
    fn calculate_shot_direction(&mut self, world: &mut ZombieWorld) {
        let player_fwd = WIELDER_FORWARD;
        let zombie_fwd = (
            f64::from(self.character.forward_direction.x),
            f64::from(self.character.forward_direction.y),
        );
        let cross_product =
            f64::from(player_fwd.0) * zombie_fwd.1 - f64::from(player_fwd.1) * zombie_fwd.0;
        let cross_product_sign = if cross_product >= 0.0 { 1.0 } else { -1.0 };
        let dot_product =
            f64::from(player_fwd.0) * zombie_fwd.0 + f64::from(player_fwd.1) * zombie_fwd.1;
        let mut angle_between = crate::java_fdlibm::acos(dot_product) * cross_product_sign;
        if angle_between < 0.0 {
            angle_between += std::f64::consts::PI * 2.0;
        }
        let deg = angle_between.to_degrees();
        // the result is not read by a melee hit; only the draws matter
        if deg < 45.0 || (180.0..225.0).contains(&deg) {
            let _ = world.rand.next(9);
        } else if (45.0..90.0).contains(&deg)
            || (135.0..180.0).contains(&deg)
            || (225.0..270.0).contains(&deg)
            || deg >= 315.0
        {
            let _ = world.rand.next(4);
        }
    }

    /// IsoGameCharacter.splatBlood(dist, alpha) -> IsoGridSquare.splatBlood: the draws of the wall splats on the
    /// west and north walls within `dist` squares (the `IsoWallBloodSplat` objects themselves are not kept).
    pub(crate) fn splat_blood(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        dist: i32,
        _alpha: f32,
    ) -> Result<(), String> {
        let Some(cur) = self.sp.current else {
            return Ok(());
        };
        let g = Sq(rd.squares);
        let _ = &g;
        let sq_at = |x: i32, y: i32, z: i32| rd.squares.get_grid_square(x, y, z);
        let this = sq_at(cur.0, cur.1, cur.2);
        let mut n = this;
        let mut w = this;
        let (tx, ty, tz) = (
            pz_math::fastfloor(cur.0 as f32),
            pz_math::fastfloor(cur.1 as f32),
            pz_math::fastfloor(cur.2 as f32),
        );
        for d_use in 0..dist {
            if n.is_some() {
                n = sq_at(tx, ty - d_use, tz);
            }
            if w.is_some() {
                w = sq_at(tx - d_use, ty, tz);
            }
            if let Some(ws) = w {
                if test_collide_adjacent_no_mover(ws, -1, 0, 0) {
                    self.splat_wall(world, rd, ws, true);
                    w = None;
                }
            }
            if let Some(ns) = n {
                if test_collide_adjacent_no_mover(ns, 0, -1, 0) {
                    self.splat_wall(world, rd, ns, false);
                    n = None;
                }
            }
        }
        Ok(())
    }

    /// The west (`west` true) or north wall part of `IsoGridSquare.splatBlood`'s loop body: which squares the splat
    /// covers and the sprite id, as draws.
    fn splat_wall(&mut self, world: &mut ZombieWorld, rd: &SpRead, sq: &IsoGridSquare, west: bool) {
        let sq_at = |x: i32, y: i32, z: i32| rd.squares.get_grid_square(x, y, z);
        // the neighbours the square's `n/s/e/w` pointers hold: the loaded squares (UNKNOWN across a blocked path)
        let (off_x, off_y) = if west { (0, 1) } else { (1, 0) };
        // west: min from `getS()`, max from `getN()`; north: min from `getW()`, max from `getE()`
        let (lo_dx, lo_dy, hi_dx, hi_dy) = if west { (0, 1, 0, -1) } else { (-1, 0, 1, 0) };
        let (tx, ty) = if west { (-1, 0) } else { (0, -1) };
        let mut min = 0;
        let mut max = 0;
        if sq_at(sq.x + lo_dx, sq.y + lo_dy, sq.z)
            .is_some_and(|s| test_collide_adjacent_no_mover(s, tx, ty, 0))
        {
            min = -1;
        }
        if sq_at(sq.x + hi_dx, sq.y + hi_dy, sq.z)
            .is_some_and(|s| test_collide_adjacent_no_mover(s, tx, ty, 0))
        {
            max = 1;
        }
        let range = max - min;
        let mut do_two = false;
        let mut start_use = 0;
        let mut end_use = 0;
        if range > 0 && world.rand.next(2) == 0 {
            do_two = true;
            if range > 1 {
                if world.rand.next(2) == 0 {
                    start_use = -1;
                    end_use = 0;
                } else {
                    start_use = 0;
                    end_use = 1;
                }
            } else {
                start_use = min;
                end_use = max;
            }
        }
        let mut off_z = world.rand.next(100) as f32 / 300.0f32;
        let a = sq_at(sq.x + off_y * start_use, sq.y + off_x * start_use, sq.z);
        let b = sq_at(sq.x + off_y * end_use, sq.y + off_x * end_use, sq.z);
        let (cut, hoppable) = if west {
            (IsoFlagType::cutW, IsoFlagType::HoppableW)
        } else {
            (IsoFlagType::cutN, IsoFlagType::HoppableN)
        };
        match (a, b) {
            (Some(a), Some(b))
                if a.has(cut)
                    && b.has(cut)
                    && !a.properties.has(IsoFlagType::WallSE)
                    && !b.properties.has(IsoFlagType::WallSE)
                    && !a.has(hoppable)
                    && !b.has(hoppable) => {}
            _ => do_two = false,
        }
        if do_two {
            let mut id = (if west { 24 } else { 28 }) + world.rand.next(2) * 2;
            if world.rand.next(2) == 0 {
                id += 8;
            }
            let _ = id;
        } else {
            let id = match world.rand.next(3) {
                0 => (if west { 0 } else { 4 }) + world.rand.next(4),
                1 => (if west { 8 } else { 12 }) + world.rand.next(4),
                _ => (if west { 16 } else { 20 }) + world.rand.next(4),
            };
            if (west && (id == 17 || id == 19)) || (!west && (id == 20 || id == 22)) {
                off_z = 0.0;
            }
            let _ = off_z;
        }
    }

    /// StateMachine.changeState(state, null, false) for one of the states a hit leads to: leave the current one,
    /// enter the new one.
    pub(crate) fn change_state(
        &mut self,
        state: ZombieState,
        world: &mut ZombieWorld,
    ) -> Result<(), String> {
        self.change_root_state(Some(state), world)
    }

    /// zombie.ai.states.StaggerBackState.enter: `stateEventDelayTimer = getMaxStaggerTime` (no network).
    pub(crate) fn stagger_back_enter(&mut self) {
        // getMaxStaggerTime: `35 * hitForce * staggerTimeMod`, clamped to 20..30
        let time = 35.0f32 * self.sp.hit_force * self.sp.stagger_time_mod;
        self.character.moving.state_event_delay_timer = time.clamp(20.0, 30.0);
    }

    /// zombie.ai.states.StaggerBackState.execute: `getAnimationPlayer().setTargetToAngle()`
    pub(crate) fn stagger_back_execute(&mut self) {
        if let Some(p) = self.sp.anim_player.as_mut() {
            p.target_angle = p.angle;
        }
    }

    /// zombie.ai.states.StaggerBackState.exit: `setStaggerBack(false)`, `setShootable(true)`,
    /// `setDeferredMovementEnabled(true)`.
    pub(crate) fn stagger_back_exit(&mut self) {
        self.sp.stagger_back = false;
        self.sp.deferred_movement_enabled = true;
    }
}

/// `BodyPartType.Head` ordinal
const BODY_PART_HEAD: i32 = 8;

/// The flags and factors `IsoZombie.Hit` is called with (`bIgnoreDamage`, `modDelta`, `bRemote`).
#[derive(Clone, Copy, Debug)]
pub struct HitModifiers {
    pub ignore_damage: bool,
    pub mod_delta: f32,
    pub remote: bool,
}

impl Default for HitModifiers {
    fn default() -> Self {
        HitModifiers {
            ignore_damage: false,
            mod_delta: 1.0,
            remote: false,
        }
    }
}
