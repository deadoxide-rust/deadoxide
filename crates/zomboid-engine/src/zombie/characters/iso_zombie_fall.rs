//! The AI states of a zombie that was knocked down: `ZombieFallDownState`, `ZombieOnGroundState` and
//! `ZombieGetUpState`, and the animation events they and `IsoGameCharacter` handle (`FallOnFront`, `SetOnFloor`,
//! `SetKnockedDown`, `IsAlmostUp`).
//!
//! NOT PORTED (UNKNOWN, each an `Err`): a dead or fake dead zombie (`die()`, the corpse), the conversion to a crawler
//! (`setCrawler(true)`, `setReanimate`: stage 3), a reanimated-for-grapple zombie, and the previous AI states of
//! `ZombieGetUpState` other than the ones a hit leads from. `isBeingSteppedOn` needs the bone positions of the lying
//! zombie (`Model.boneToWorldCoords`, not ported): it is answered only while the player stand-in is too far away to
//! stand on it.
use super::iso_zombie::{IsoZombie, ZombieState, ZombieWorld};
use super::iso_zombie_sp::SpRead;
use crate::zombie::core::random::rand_abstract::RandAbstract;

/// The farthest a point `DoCollisionBoneCheck` tests can be from the zombie's position: a bone of a lying skeleton
/// (spine, calves, head: at most 1.3 from its root) plus the largest radius it tests (0.32) plus the wielder's reach
/// along its forward direction (0.3). A player farther away cannot be standing on the zombie.
const STEPPED_ON_REACH: f32 = 1.3 + 0.32 + 0.3;

impl IsoZombie {
    /// IsoGameCharacter.isBeingSteppedOn for the single-player zombie: `false` unless it lies on the floor and the
    /// player stand-in is close enough to test its bones (`Err` then: the pose is not ported).
    pub(crate) fn is_being_stepped_on(&self, rd: &SpRead) -> Result<bool, String> {
        if !self.on_floor {
            return Ok(false);
        }
        let p = &rd.world.player;
        let m = &self.character.moving;
        let dx = f64::from(p.x - m.x);
        let dy = f64::from(p.y - m.y);
        let dist = (dx * dx + dy * dy).sqrt() as f32;
        if dist < STEPPED_ON_REACH {
            return Err(format!(
                "IsoGameCharacter.isBeingSteppedOn: the player is {dist} from the lying zombie, its bones' world positions are not ported"
            ));
        }
        Ok(false)
    }

    /// zombie.characters.IsoZombie.setEatBodyTarget(null, force): the overload without a speed evaluates
    /// `Rand.Next(0.64F, 0.96F)` for its `eatSpeed` argument whatever the target is. The zombie has no eat body
    /// target (a corpse to eat is not ported), so `target != this.eatBodyTarget` is false and nothing else changes.
    pub(crate) fn set_eat_body_target_null(&mut self, world: &mut ZombieWorld) {
        world.rand.next_float(0.64, 0.96);
    }

    /// zombie.ai.states.ZombieFallDownState.enter
    pub(crate) fn fall_down_enter(&mut self, world: &mut ZombieWorld) {
        self.sp.block_turning = true;
        self.sp.stagger_back = false;
        // `ownerZombie.isAlive()`
        if !self.is_dead() {
            self.sp.hit_reaction = String::new();
        }
        self.set_eat_body_target_null(world);
        // setSitAgainstWall(false): no state any ported code reads
    }

    /// zombie.ai.states.ZombieFallDownState.exit
    pub(crate) fn fall_down_exit(&mut self) {
        self.sp.block_turning = false;
        self.on_floor = true;
    }

    /// zombie.ai.states.ZombieOnGroundState.enter
    pub(crate) fn on_ground_enter(&mut self, world: &mut ZombieWorld) -> Result<(), String> {
        // setCollidable(false): `IsoZombie.updateInternal` sets it back to true before anything reads it
        if self.is_dead() {
            // `!isDead()` guards `setOnFloor(true)` and the whole branch: `die()` (the corpse)
            return self.die(world);
        }
        self.on_floor = true;
        // `!isDead() && !isFakeDead()`
        self.sp.stagger_back = false;
        self.sp.knocked_down = false;
        self.sp.hit_reaction = String::new();
        self.set_eat_body_target_null(world);
        // setSitAgainstWall(false)
        if !self.sp.become_crawler {
            // "Tutorial" game mode is not the mode: startReanimateTimer
            start_reanimate_timer(self, &mut world.rand);
            // parameterZombieState.setState(Idle): an FMOD parameter
        }
        Ok(())
    }

    /// zombie.ai.states.ZombieOnGroundState.execute
    pub(crate) fn on_ground_execute(
        &mut self,
        world: &mut ZombieWorld,
        rd: &SpRead,
    ) -> Result<(), String> {
        if self.is_dead() {
            // `!isRagdollSimulationActive()` (no ragdoll is simulated): `die()`
            return self.die(world);
        }
        if self.sp.become_crawler {
            // isBeingSteppedOn / isUnderVehicle, then setCrawler(true), setCanWalk(false), setReanimate(true)
            if !self.is_being_stepped_on(rd)? {
                return Err(
                    "ZombieOnGroundState.execute: the conversion to a crawler (setCrawler) is not ported".into(),
                );
            }
            return Ok(());
        }
        if let Some(p) = self.sp.anim_player.as_mut() {
            p.target_angle = p.angle;
        }
        self.sp.reanimate_timer -= world.thirty_fps_multiplier();
        if self.sp.reanimate_timer <= 2.0 {
            // getReanimatedPlayer() is null
            if self.is_being_stepped_on(rd)? {
                start_reanimate_timer(self, &mut world.rand);
            }
            // `!isLocal() && ...`: the zombie is local
        }
        Ok(())
    }

    /// zombie.ai.states.ZombieGetUpState.enter: remembers the previous AI state, `setOnFloor(true)`.
    pub(crate) fn get_up_enter(&mut self) -> Result<(), String> {
        let previous = self.sp.previous_state;
        match previous {
            Some(ZombieState::OnGround) | Some(ZombieState::FallDown) | None => {}
            Some(other) => {
                return Err(format!(
                    "ZombieGetUpState: getting up from {} is not ported",
                    other.name()
                ))
            }
        }
        self.sp.get_up_prev_state = previous;
        self.on_floor = true;
        Ok(())
    }

    /// zombie.ai.states.ZombieGetUpState.exit
    pub(crate) fn get_up_exit(&mut self) {
        // setCollidable(true)
        self.sp.plain.clear_variable("SprinterTripped");
        self.sp.plain.clear_variable("ShouldStandUp");
        // `isNullOrEmpty(getHitReaction())`: setSitAgainstWall(false)
        self.sp.knocked_down = false;
        self.allow_repath_delay = 0.0;
        // PREV_STATE PathFind / WalkToward: refused on entry
    }
}

/// zombie.ai.states.ZombieOnGroundState.startReanimateTimer: `Rand.Next(60) + 30`
fn start_reanimate_timer(z: &mut IsoZombie, rand: &mut RandAbstract<crate::java::Random>) {
    z.sp.reanimate_timer = (rand.next(60) + 30) as f32;
}
