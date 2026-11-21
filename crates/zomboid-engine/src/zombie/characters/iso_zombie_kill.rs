//! The kill of the single-player zombie by a melee hit: `IsoGameCharacter.Kill`/`onKilled`/`DoDeath`/
//! `doDeathSplatterAndSounds`, `IsoZombie.onKilled`/`DoZombieInventory`, then `die()` and `becomeCorpse()` (the
//! `IsoDeadBody(died)` the zombie leaves on its square).
//!
//! What the original does that no state of this port keeps, and so what is only reproduced as the random draws it
//! makes: the blood splats on the walls (`IsoGridSquare.splatBlood`) and the floor (`IsoChunk.addBloodSplat`), the
//! `IsoZombieGiblets` objects (their constructors draw `Rand.Next(4000)` twice and `Rand.Next(3)`), the sounds (a
//! random clip of the sound script), the wielder's XP and the Lua events `OnZombieDead`, `OnCharacterDeath`,
//! `OnContainerUpdate` and `OnDeadBodySpawn` (no handler of the game's own Lua draws or changes anything measured).
//!
//! NOT PORTED (UNKNOWN, each an `Err`): a dead zombie that no hit killed (`die()` would `Kill(getAttackedBy())`), a
//! zombie that wears or carries items (`DoZombieInventory` makes them: the harness zombie is naked), a corpse of a
//! crawling, burning or reanimated zombie, a weapon of the long blade category (`isKilledBySlicingWeapon` reads the
//! wielder's attacking weapon), the radial blood splash of bare hands (`weapon == player.bareHands`: the stand-in's
//! own bare hands are not the item a scenario makes), the blood on the corpse's `HumanVisual` (`addBlood` is not
//! ported, so the copy is the zombie's visual as it was), `deathTime` and the corpse's save.
use super::iso_zombie::{IsoZombie, ZombieWorld};
use super::iso_zombie_hit::HitWeapon;
use super::iso_zombie_sp::{play_sound_impl, SpRead};
use crate::zombie::inventory::item_container::ItemContainer;
use crate::zombie::iso::iso_moving_object::IsoMovingObject;
use crate::zombie::iso::objects::iso_dead_body::IsoDeadBody;
use crate::zombie::iso::vector2::Vector2;

/// The `IsoZombieGiblets` constructor: `velX += Rand.Next(4000) / 10000.0F - 0.2F` for both axes, then the sprite is
/// hidden when `Rand.Next(3) != 0`. Only the draws matter (the object is never updated or read).
fn make_giblets(world: &mut ZombieWorld) {
    let _ = world.rand.next(4000);
    let _ = world.rand.next(4000);
    let _ = world.rand.next(3);
}

/// A corpse `new IsoDeadBody(died)` made, with what the original reads off the square it is put on.
#[derive(Clone, Debug)]
pub struct Corpse {
    pub body: IsoDeadBody,
    /// `died.getCurrentSquare()`: the square whose `staticMovingObjects` the corpse joins
    pub square: Option<(i32, i32, i32)>,
    /// `died.getForwardDirection(this.forwardDirection)`
    pub forward: Vector2,
}

impl IsoZombie {
    /// IsoGameCharacter.Kill(killer, attackingWeapon, isGory, onDiedListener), as `Kill(weapon, wielder)` calls it
    /// from `hitConsequences` (`!GameServer.server && !GameClient.client`).
    pub(crate) fn kill(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
    ) -> Result<(), String> {
        // setAttackedBy(killer), addOnDiedListener(null, true): the stand-in; nothing
        self.character.health = 0.0;
        // `getBodyDamage()` is null for a zombie
        if !self.sp.on_kill_done {
            self.on_killed(world, rd, weapon, true)?;
            self.sp.on_kill_done = true;
        }
        Ok(())
    }

    /// IsoZombie.onKilled(killer, handWeapon, bGory): `DoZombieInventory`, `OnZombieDead`, then `DoDeath` with the
    /// weapon of the hit.
    fn on_killed(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
        gory: bool,
    ) -> Result<(), String> {
        // IsoGameCharacter.onKilled is empty; `GameServer.server` is false; DoZombieInventory: the harness zombie
        // wears and carries nothing (the corpse's item counts are compared), so the container stays empty
        // LuaEventManager.triggerEvent("OnZombieDead", this)
        self.do_death(world, rd, weapon, gory)
    }

    /// IsoGameCharacter.DoDeath(weapon, wielder, isGory): `OnDeath()` (the Lua event `OnCharacterDeath`), the logs and
    /// the chat of a server (none here), `doDeathSplatterAndSounds`.
    fn do_death(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
        gory: bool,
    ) -> Result<(), String> {
        self.do_death_splatter_and_sounds(world, rd, weapon, gory)
    }

    /// IsoGameCharacter.playDeadSound for a zombie of the single-player game: `HeadStab` when it was killed close,
    /// `HeadSlice` when killed by a slicing weapon, else `HeadSmash`; then the FMOD parameter (no state).
    fn play_dead_sound(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
    ) -> Result<(), String> {
        // `isCloseKilled()` is false; `isKilledBySlicingWeapon()` reads `killer.getAttackingWeapon()` for the
        // `LONG_BLADE` category
        if weapon.is_of_category("longblade") {
            return Err(
                "IsoGameCharacter.isKilledBySlicingWeapon: a long blade weapon is not ported"
                    .into(),
            );
        }
        play_sound_impl(
            rd.world.game_sounds.as_ref(),
            Some("HeadSmash"),
            &mut world.rand,
        )
    }

    /// IsoGameCharacter.doDeathSplatterAndSounds(weapon, wielder, isGory) for a zombie that is dead.
    fn do_death_splatter_and_sounds(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
        weapon: &HitWeapon,
        gory: bool,
    ) -> Result<(), String> {
        // onDeath_ShouldDoSplatterAndSounds: `!isReanimatedForGrappleOnly()`
        if !self.sp.death_sound_done {
            self.play_dead_sound(world, rd, weapon)?;
        }
        self.sp.death_sound_done = true;
        if !self.is_dead() {
            return Ok(());
        }
        let blood_level = rd.world.blood_level;
        // the giblets' height `dz` only places them
        if blood_level > 1 && gory {
            let mut spn = weapon.splat_number;
            if spn < 1 {
                spn = 1;
            }
            // Core.lastStand is false
            match blood_level {
                4 => spn *= 2,
                5 => spn *= 5,
                2 => spn /= 2,
                _ => {}
            }
            for _ in 0..spn {
                self.splat_blood(world, rd, 3, 0.3f32)?;
            }
            // splatBloodFloorBig: the chunk of the zombie's square gets a floor splat of `Rand.Next(20)`
            if self.sp.current.is_some() {
                let _ = world.rand.next(20);
            }
            // wielder.xp.AddXP(weapon, 3): the stand-in's experience
        }
        if blood_level > 1 && gory {
            // `isOnFloor() && wielder instanceof IsoPlayer && weapon == player.bareHands` (the radial splash) is not
            // the stand-in's item (see the module comment); the other branch:
            self.play_blood_splatter_sound(world, rd)?;
            make_giblets(world);
            let (mut rand, mut rand2, mut repeat) = (3, 0, 1);
            match blood_level {
                1 => repeat = 0,
                2 => {
                    repeat = 1;
                    rand = 5;
                    rand2 = 2;
                }
                4 => {
                    repeat = 3;
                    rand = 2;
                }
                5 => {
                    repeat = 10;
                    rand = 0;
                }
                _ => {}
            }
            // `isCloseKilled()` is false
            for _ in 0..repeat {
                for _ in 0..4 {
                    if world.rand.next(rand) == 0 {
                        make_giblets(world);
                    }
                }
                for _ in 0..6 {
                    if world.rand.next(rand2) == 0 {
                        make_giblets(world);
                    }
                }
                // the eye
                if world.rand.next(6) == 0 {
                    make_giblets(world);
                }
            }
        }
        Ok(())
    }

    /// IsoGameCharacter.playBloodSplatterSound: `getEmitter().playSoundImpl("BloodSplatter", this)`.
    fn play_blood_splatter_sound(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
    ) -> Result<(), String> {
        play_sound_impl(
            rd.world.game_sounds.as_ref(),
            Some("BloodSplatter"),
            &mut world.rand,
        )
    }

    /// IsoGameCharacter.die(): `Kill(getAttackedBy())` when not killed yet, then `becomeCorpse()` (no client, no
    /// server), then `onDeathDone`.
    pub(crate) fn die(&mut self, world: &mut ZombieWorld) -> Result<(), String> {
        if !self.sp.on_death_done || !self.sp.died_body {
            if !self.sp.on_kill_done {
                return Err(
                    "IsoGameCharacter.die: a zombie that no hit killed (Kill(getAttackedBy())) is not ported"
                        .into(),
                );
            }
            self.become_corpse(world)?;
            self.sp.on_death_done = true;
        }
        Ok(())
    }

    /// IsoGameCharacter.becomeCorpse: `diedBody = new IsoDeadBody(this)` unless there is one.
    fn become_corpse(&mut self, world: &mut ZombieWorld) -> Result<(), String> {
        if self.sp.died_body {
            return Ok(());
        }
        let corpse = self.new_dead_body()?;
        let sp = world
            .sp
            .as_mut()
            .ok_or("IsoGameCharacter.becomeCorpse: the world has no single-player part")?;
        sp.corpses.push(corpse);
        self.sp.died_body = true;
        Ok(())
    }

    /// zombie.iso.objects.IsoDeadBody(died, wasCorpseAlready false, bAddToSquareAndWorld true) for a zombie: what it
    /// copies off the dead character. Nothing in it draws a random number (`pathToLocationF` of nearby zombies is for
    /// a corpse that was not a zombie).
    fn new_dead_body(&mut self) -> Result<Corpse, String> {
        if self.crawling {
            return Err("IsoDeadBody: the corpse of a crawling zombie is not ported".into());
        }
        let m = &self.character.moving;
        let mut moving = IsoMovingObject::at(m.x, m.y, m.z);
        moving.offset_x = m.offset_x;
        moving.offset_y = m.offset_y;
        moving.dir = self.character.forward_iso_direction().ordinal() as i32;
        let human_visual = self.character.human_visual.clone();
        let body = IsoDeadBody {
            moving,
            female: self.character.female,
            was_zombie: true,
            object_id: (0, 0),
            server: false,
            persistent_outfit_id: 0,
            desc: self.character.descriptor.clone(),
            zombie_rot_stage_at_death: human_visual.zombie_rot_stage,
            human_visual,
            container: Some(ItemContainer::default()),
            worn_items: Vec::new(),
            attached_items: Vec::new(),
            death_time: 0.0,
            reanimate_time: 0.0,
            fall_on_front: self.sp.fall_on_front,
            killed_by_fall: false,
            was_skeleton: false,
            angle: self.anim_angle_radians(),
            animal_rot_stage_at_death: 0,
            rotten_texture: None,
            skel_inv_icon: None,
            crawling: false,
            fake_dead: false,
            ragdoll_fall: false,
            died_bone_transforms: Vec::new(),
        };
        Ok(Corpse {
            body,
            square: self.sp.current,
            forward: self.character.forward_direction,
        })
    }
}
