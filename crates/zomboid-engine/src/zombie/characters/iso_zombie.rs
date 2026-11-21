//! zombie.characters.IsoZombie: construction (descriptor, look, stats, speed) of a zombie, in the original's
//! operation order and with its random draws.
//!
//! What is here is measured against the original run headless (`pz-dump zombie` vs `ZombieProbe`); the rest of the
//! 5,976-line class (states, targeting, hearing, attacks, outfits ...) is not ported yet.
use std::rc::Rc;

use super::action::action_context::ActionContext;
use super::action::action_group::ActionGroup;
use super::iso_game_character::IsoGameCharacterBase;
use super::survivor_factory::SurvivorFactory;
use crate::java::{self, Random};
use crate::zombie::core::math::pz_math;
use crate::zombie::core::random::rand_abstract::RandAbstract;
use crate::zombie::core::skinnedmodel::advancedanimation::advanced_animator::{
    AdvancedAnimator, AnimatorOwner, Env,
};
use crate::zombie::core::skinnedmodel::advancedanimation::anim_event::{AnimEvent, AnimEventKind};
use crate::zombie::core::skinnedmodel::advancedanimation::animation_set::AnimationSet;
use crate::zombie::core::skinnedmodel::advancedanimation::animation_variable_reference::AnimationVariableReference;
use crate::zombie::core::skinnedmodel::advancedanimation::animation_variable_source::{
    AnimationVariableHandlePool, AnimationVariableSlot, AnimationVariableSource,
};
use crate::zombie::core::skinnedmodel::advancedanimation::i_animation_variable_source::IAnimationVariableSource;
use crate::zombie::core::skinnedmodel::animation::animation_clips::AnimationClips;
use crate::zombie::core::skinnedmodel::animation::animation_player::AnimationPlayer;
use crate::zombie::core::skinnedmodel::animation::animation_track::AnimationTrack;
use crate::zombie::core::skinnedmodel::population::outfit_rng::OutfitRng;
use crate::zombie::iso::iso_directions::IsoDirections;
use crate::zombie::iso::iso_grid_square::GetSquare;
use crate::zombie::iso::sprite_details::iso_flag_type::IsoFlagType;
use crate::zombie::iso::vector2::Vector2;
use crate::zombie::pathfind::path_find_behavior2::PathFindBehavior2;
use crate::zombie::sandbox_options::SandboxOptions;
use crate::zombie::util::frame_delay::FrameDelay;
use crate::zombie::util::string_utils;

/// The `SandboxOptions.instance.lore` values the zombie code reads, as the integers `getValue()` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lore {
    pub speed: i32,
    pub sprinter_percentage: i32,
    pub strength: i32,
    pub cognition: i32,
    pub door_opening_percentage: i32,
    pub crawl_under_vehicle: i32,
    pub memory: i32,
    pub sight: i32,
    pub hearing: i32,
    pub active_only: i32,
}

impl Lore {
    /// The values `SandboxOptions.instance.lore.<option>.getValue()` returns, by option name ("ZombieLore.Speed").
    pub fn from_sandbox(sandbox: &SandboxOptions) -> Result<Self, String> {
        let get = |name: &str| {
            sandbox
                .get_int(&format!("ZombieLore.{name}"))
                .ok_or_else(|| format!("sandbox option ZombieLore.{name} is not an integer option"))
        };
        Ok(Lore {
            speed: get("Speed")?,
            sprinter_percentage: get("SprinterPercentage")?,
            strength: get("Strength")?,
            cognition: get("Cognition")?,
            door_opening_percentage: get("DoorOpeningPercentage")?,
            crawl_under_vehicle: get("CrawlUnderVehicle")?,
            memory: get("Memory")?,
            sight: get("Sight")?,
            hearing: get("Hearing")?,
            active_only: get("ActiveOnly")?,
        })
    }
}

/// What the original's singletons give the zombie code, made explicit: the global `Rand`, the thread's `OutfitRNG`,
/// `SandboxOptions.instance.lore`, `SurvivorFactory`'s lists and `SurvivorDesc.idCount`.
pub struct ZombieWorld {
    pub rand: RandAbstract<Random>,
    pub outfit_rng: OutfitRng,
    pub lore: Lore,
    pub survivors: SurvivorFactory,
    pub survivor_id_count: i32,
    /// `GameTime.instance.getMultiplier()` (0.8 on a dedicated server at the default frame multipliers)
    pub multiplier: f32,
    /// `RainManager.isRaining()`
    pub raining: bool,
    /// The game's own animation data a zombie that ticks is built over; a zombie made without it cannot tick.
    pub anim_assets: Option<ZombieAnimAssets>,
    /// `Some` for the single-player game (`GameServer.server` false): the player the zombie can target and the loaded
    /// chunks. `None` is the dedicated server's world, where the zombie has no variables and no target.
    pub sp: Option<super::iso_zombie_sp::SinglePlayerWorld>,
}

/// What `ActionGroup.getActionGroup("zombie")`, `AnimationSet.GetAnimationSet("zombie")` and the skeleton's animation
/// clips are, loaded once from the player's own files.
#[derive(Clone)]
pub struct ZombieAnimAssets {
    pub action_group: Rc<ActionGroup>,
    pub anim_set: Rc<AnimationSet>,
    pub clips: Rc<AnimationClips>,
    /// `AdvancedAnimator.motionScale` and `rotationScale` (`media/AnimSets/Defaults.xml`)
    pub motion_scale: f32,
    pub rotation_scale: f32,
}

/// The zombie's `StateMachineComponent` pieces (its `ActionContext` and `AdvancedAnimator`) with the animation fields
/// `IsoGameCharacter` keeps for them.
pub struct ZombieAnim {
    pub action_context: ActionContext,
    pub animator: AdvancedAnimator,
    /// `getGameVariablesInternal()`: stays empty, `setVariable` does nothing for a zombie on a dedicated server
    pub game_variables: AnimationVariableSource,
    animation_invisible_frame_delay: FrameDelay,
    /// `animationTimeScale`
    pub animation_time_scale: f32,
    /// `animationUpdatingThisFrame`
    pub animation_updating_this_frame: bool,
}

/// The names `IsoGameCharacter` registers anim event handlers for (`registerAnimEventListeners`).
pub(crate) const ANIM_EVENT_HANDLERS: [&str; 15] = [
    "SetVariable",
    "ClearVariable",
    "PlaySound",
    "PlaySoundNoBlend",
    "Footstep",
    "DamageWhileInTrees",
    "TurnAround",
    "TurnAround_FlipSkeleton",
    "SetSharedGrappleType",
    "GrapplerLetGo",
    "FallOnFront",
    "SetOnFloor",
    "SetKnockedDown",
    "IsAlmostUp",
    "KilledByAttacker",
];

/// The zombie as the animator's character: variables (the action context's state variables, then the game variables)
/// and the anim event callback.
struct ZombieOwner<'a> {
    ctx: &'a mut ActionContext,
    vars: &'a AnimationVariableSource,
}

impl IAnimationVariableSource for ZombieOwner<'_> {
    /// IsoGameCharacter.getVariable: the action context's state variable first.
    fn get_variable(&self, key: &str) -> Option<AnimationVariableSlot> {
        self.ctx
            .get_variable(key)
            .or_else(|| self.vars.get_variable(key).cloned())
    }
}

impl AnimatorOwner for ZombieOwner<'_> {
    fn is_player_ready(&self) -> bool {
        true
    }

    /// IsoGameCharacter.OnAnimEvent: the handler registered for the event's name, then `ActionContext.reportEvent`,
    /// then the AI state's `animEvent` (`ZombieIdleState` has none to run).
    fn on_anim_event(
        &mut self,
        _layer_depth: i32,
        layer_state_name: Option<&str>,
        _track: Option<&AnimationTrack>,
        event: &AnimEvent,
        _rand: &mut RandAbstract<Random>,
    ) -> Result<(), String> {
        let Some(name) = event.event_name.as_deref() else {
            return Ok(());
        };
        if string_utils::is_null_or_whitespace(Some(name)) {
            return Ok(());
        }
        if ANIM_EVENT_HANDLERS
            .iter()
            .any(|h| java::equals_ignore_case(h, name))
        {
            // SetVariable / ClearVariable on a zombie of a dedicated server change nothing unless the variable is
            // an action state's (a shared slot: not reproduced)
            let reference = match (&event.kind, name.to_ascii_lowercase().as_str()) {
                (
                    AnimEventKind::SetVariable {
                        variable_reference, ..
                    },
                    "setvariable",
                ) => Some(variable_reference.clone()),
                (_, "clearvariable") => event
                    .parameter_value
                    .as_deref()
                    .map(AnimationVariableReference::from_raw_variable_name),
                _ => None,
            };
            match reference {
                Some(r) if r.get_variable(&*self).is_none() => {}
                _ => {
                    return Err(format!(
                        "IsoGameCharacter.OnAnimEvent: the handler of '{name}' is not ported"
                    ))
                }
            }
        }
        self.ctx.report_event_for_state(layer_state_name, name);
        Ok(())
    }

    /// AnimationVariableReference.setVariable(character, bool): a slot of the action state would be set; otherwise the
    /// character's `setVariable(name, value)`, which a zombie of a dedicated server ignores.
    fn set_variable_bool(
        &mut self,
        reference: &AnimationVariableReference,
        _value: bool,
    ) -> Result<(), String> {
        if reference.get_variable(&*self).is_some() {
            return Err(
                "AnimationVariableReference.setVariable: an action state's variable slot is not ported"
                    .into(),
            );
        }
        Ok(())
    }

    fn set_variable_float(&mut self, key: &str, _value: f32) -> Result<(), String> {
        Err(format!(
            "IAnimationVariableSlot.setValue(float) of '{key}': not ported"
        ))
    }
}

impl ZombieWorld {
    /// zombie.GameTime.getThirtyFPSMultiplier: `getMultiplier() / 1.6F`
    pub fn thirty_fps_multiplier(&self) -> f32 {
        self.multiplier / 1.6
    }

    /// zombie.GameTime.getTimeDelta: `getTimeDeltaFromMultiplier(getMultiplier())`, `multiplier / 0.8F /
    /// multiplierBias / 60.0F` (`multiplierBias` 1.0).
    pub fn time_delta(&self) -> f32 {
        self.multiplier / 0.8 / 1.0 / 60.0
    }
}

/// The AI states a zombie is in so far as this port goes (zombie.ai.states.*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZombieState {
    /// zombie.ai.states.ZombieIdleState
    Idle,
    /// zombie.ai.states.PathFindState
    PathFind,
    /// zombie.ai.states.LungeState
    Lunge,
    /// zombie.ai.states.AttackState
    Attack,
    /// zombie.ai.states.ZombieFaceTargetState
    FaceTarget,
    /// zombie.ai.states.WalkTowardState
    WalkToward,
    /// zombie.ai.states.StaggerBackState
    StaggerBack,
    /// zombie.ai.states.ZombieFallDownState
    FallDown,
    /// zombie.ai.states.ZombieOnGroundState
    OnGround,
    /// zombie.ai.states.ZombieGetUpState
    GetUp,
}

impl ZombieState {
    /// `getClass().getSimpleName()`
    pub fn name(self) -> &'static str {
        match self {
            ZombieState::Idle => "ZombieIdleState",
            ZombieState::PathFind => "PathFindState",
            ZombieState::Lunge => "LungeState",
            ZombieState::Attack => "AttackState",
            ZombieState::FaceTarget => "ZombieFaceTargetState",
            ZombieState::WalkToward => "WalkTowardState",
            ZombieState::StaggerBack => "StaggerBackState",
            ZombieState::FallDown => "ZombieFallDownState",
            ZombieState::OnGround => "ZombieOnGroundState",
            ZombieState::GetUp => "ZombieGetUpState",
        }
    }
}

/// zombie.characters.IsoZombie, the fields construction sets or reads.
pub struct IsoZombie {
    pub character: IsoGameCharacterBase,
    /// `palette` (final)
    pub palette: i32,
    pub sprite_name: String,
    /// `sprite.def.tintr/tintg/tintb`
    pub tint: [f32; 3],
    pub zombie_id: i32,
    pub hurt_sound: String,
    pub speed_type: i32,
    pub strength: i32,
    pub cognition: i32,
    pub memory: i32,
    pub sight: i32,
    pub hearing: i32,
    pub walk_type: Option<String>,
    pub walk_variant: String,
    /// `speedMod` (IsoGameCharacter, initialiser 1.0): set by the speed routines
    pub speed_mod: f32,
    pub lunger: bool,
    pub running: bool,
    pub crawling: bool,
    pub inactive: bool,
    pub can_crawl_under_vehicle: bool,
    /// `StateMachine.currentState` (`None` until the action system first enters a state)
    pub current_state: Option<ZombieState>,
    /// ZombieIdleState.TICK_COUNT
    pub idle_tick_count: i64,
    pub time_since_seen_flesh: f32,
    pub time_since_respond_to_sound: f32,
    pub check_for_corpse_timer: f32,
    pub allow_repath_delay: f32,
    pub bonus_spot_time: f32,
    pub movex: f32,
    pub movey: f32,
    pub on_floor: bool,
    pub path: PathFindBehavior2,
    /// The action system and animator; `None` for a zombie made without the animation assets (it cannot tick).
    pub anim: Option<ZombieAnim>,
    /// The single-player zombie's state (`GameServer.server` false when it was built): see `iso_zombie_sp`.
    pub sp: SpState,
}

/// What the single-player zombie keeps besides the dedicated server's: its target, its variables (the 164
/// `registerVariableCallbacks` registers and the plain ones states set), the animation player's angles, the movement and
/// turning flags and the state parameters.
pub struct SpState {
    /// `GameServer.server` was false when the zombie was built (its variable callbacks exist)
    pub enabled: bool,
    /// `target` is the player stand-in
    pub target: bool,
    /// `vectorToTarget`
    pub vector_to_target: Vector2,
    /// the slots `setVariable` made that are not one of the registered callbacks (and the two plain registered ones)
    pub plain: AnimationVariableSource,
    /// the handle pool the plain slots are named by
    pub pool: AnimationVariableHandlePool,
    /// `animPlayer`, made by the first `getAnimationPlayer()` (the first animation update frame)
    pub anim_player: Option<AnimationPlayer>,
    /// `isMoving`
    pub is_moving: bool,
    pub is_turning: bool,
    pub is_turning90: bool,
    pub is_turning_around: bool,
    /// `initialTurningAroundTarget` (degrees)
    pub initial_turning_around_target: f32,
    /// `maxTwist` (degrees, 15) and `moveDelta`
    pub max_twist: f32,
    pub move_delta: f32,
    /// `knockedDown`
    pub knocked_down: bool,
    /// IsoZombie `hitTime` (hits taken), IsoGameCharacter `hitForce`, `staggerTimeMod`, `hitDir`, `hitReaction`,
    /// IsoZombie `staggerBack`, `becomeCrawler`, `usePhysicHitReaction`, `fallOnFront`, `lastHitPart` (BodyPartType
    /// ordinal)
    pub hit_time: i32,
    pub hit_force: f32,
    pub stagger_time_mod: f32,
    pub hit_dir: Vector2,
    pub hit_reaction: String,
    pub stagger_back: bool,
    pub become_crawler: bool,
    pub use_physic_hit_reaction: bool,
    pub fall_on_front: bool,
    pub last_hit_part: i32,
    /// IsoGameCharacter `blockTurning` (set while falling down) and `reanimateTimer`; ZombieGetUpState's `PREV_STATE`
    pub block_turning: bool,
    pub reanimate_timer: f32,
    /// `onKillDone`, `onDeathDone` (`Kill`/`die`), and the negation of `doDeathSound` (initialiser true)
    pub on_kill_done: bool,
    pub on_death_done: bool,
    pub death_sound_done: bool,
    /// `diedBody != null`: the corpse `becomeCorpse` made is in `SinglePlayerWorld.corpses`
    pub died_body: bool,
    pub get_up_prev_state: Option<ZombieState>,
    /// `canSeeTarget`
    pub can_see_target: bool,
    pub alerted: bool,
    pub lunge_timer: f32,
    pub target_seen_time: f32,
    /// `StateMachine.previousState`
    pub previous_state: Option<ZombieState>,
    /// `getActionContext().peekPreviousStateName()` as of the last change of the action states
    pub previous_action_state: String,
    /// `walkVariantUse`
    pub walk_variant_use: Option<String>,
    /// `lastTargetSeenX` (-1: none)
    pub last_target_seen_x: i32,
    /// `attackOutcome`, `attackDidDamage`
    pub attack_outcome: String,
    pub attack_did_damage: bool,
    /// the AI states' TICK_COUNT params
    pub path_find_tick_count: i64,
    pub lunge_tick_count: i64,
    /// WalkTowardState's `IGNORE_OFFSET` (None until the state is first entered) and `TICK_COUNT`
    pub walk_toward_ignore_offset: Option<bool>,
    pub walk_toward_tick_count: i64,
    /// IsoMovingObject `nx`/`ny` (`getNextX/Y`: where the zombie goes this tick), `lastX`/`ly`/`lz` (as of the last
    /// `update`)
    pub next_x: f32,
    pub next_y: f32,
    pub last_x: f32,
    pub last_y: f32,
    pub last_z: f32,
    /// IsoMovingObject `current` and `last`: the squares by coordinates (`None` is null)
    pub current: Option<(i32, i32, i32)>,
    pub last: Option<(i32, i32, i32)>,
    /// IsoMovingObject `collidedN/S/W/E`, `collidedThisFrame`, `altCollide`, `timeSinceZombieAttack` (1000000)
    pub collided_n: bool,
    pub collided_s: bool,
    pub collided_w: bool,
    pub collided_e: bool,
    pub collided_this_frame: bool,
    pub alt_collide: bool,
    /// The tile objects (square x, y, z and index in its object list) whose `IsoObject.objectRenderEffects` is set: the
    /// zombie's walk made them rustle and the headless world never updates them, so the effect stays.
    pub render_effects: Vec<(i32, i32, i32, usize)>,
    pub time_since_zombie_attack: i32,
    /// IsoGameCharacter `deferredMovementEnabled` (true until a state turns it off)
    pub deferred_movement_enabled: bool,
}

impl Default for SpState {
    fn default() -> Self {
        SpState {
            enabled: false,
            target: false,
            vector_to_target: Vector2::default(),
            plain: AnimationVariableSource::default(),
            pool: AnimationVariableHandlePool::default(),
            anim_player: None,
            is_moving: false,
            is_turning: false,
            is_turning90: false,
            is_turning_around: false,
            initial_turning_around_target: 0.0,
            max_twist: 15.0,
            move_delta: 1.0,
            knocked_down: false,
            hit_time: 0,
            hit_force: 0.0,
            stagger_time_mod: 0.0,
            hit_dir: Vector2::default(),
            hit_reaction: String::new(),
            stagger_back: false,
            become_crawler: false,
            use_physic_hit_reaction: false,
            fall_on_front: false,
            last_hit_part: 0,
            block_turning: false,
            reanimate_timer: 0.0,
            on_kill_done: false,
            on_death_done: false,
            death_sound_done: false,
            died_body: false,
            get_up_prev_state: None,
            can_see_target: false,
            alerted: false,
            lunge_timer: 0.0,
            target_seen_time: 0.0,
            previous_state: None,
            previous_action_state: "idle".to_string(),
            walk_variant_use: None,
            last_target_seen_x: -1,
            attack_outcome: String::new(),
            attack_did_damage: false,
            path_find_tick_count: 0,
            lunge_tick_count: 0,
            walk_toward_ignore_offset: None,
            walk_toward_tick_count: 0,
            next_x: 0.0,
            next_y: 0.0,
            last_x: 0.0,
            last_y: 0.0,
            last_z: 0.0,
            current: None,
            last: None,
            collided_n: false,
            collided_s: false,
            collided_w: false,
            collided_e: false,
            collided_this_frame: false,
            alt_collide: false,
            render_effects: Vec::new(),
            time_since_zombie_attack: 1_000_000,
            deferred_movement_enabled: true,
        }
    }
}

impl IsoZombie {
    /// zombie.characters.IsoZombie(IsoCell cell): `this(cell, null, -1)`, so the descriptor is made by
    /// `SurvivorFactory.CreateSurvivor()` and the palette is `Rand.Next(3) + 1`. `Err` where the original would throw
    /// or where a path is not ported (an `ActiveOnly` other than "both", which needs the climate's dawn and dusk).
    ///
    /// Not modelled: the cell and world registration, the grapple/died listeners and variable callbacks, the sprite
    /// instance's animation fields (`def.animFrameIncrease`, `setFrameSpeedPerFrame`: no draws), `setDefaultState`,
    /// `initializeStates`, the action group, worn and attached items, the aggro list and the FMOD parameters.
    pub fn new(world: &mut ZombieWorld) -> Result<Self, String> {
        // super(cell, 0.0F, 0.0F, 0.0F): IsoGameCharacter(...) draws the patience
        let mut character = IsoGameCharacterBase::new(&mut world.rand, 0.0, 0.0, 0.0);
        character.health = 1.8 + world.rand.next_float(0.0, 0.3);
        character.moving.weight = 0.7;
        let direction = IsoDirections::get_random(&mut world.rand);
        character.set_forward_iso_direction(direction)?;
        character.human_visual.random_blood(&mut world.outfit_rng);
        let descriptor = world
            .survivors
            .create_survivor(&mut world.rand, &mut world.survivor_id_count)?;
        let palette = world.rand.next(3) + 1;
        character.descriptor = Some(descriptor);
        let female = character.descriptor.as_ref().is_some_and(|d| d.female);
        character.set_female(female);
        if let Some(d) = character.descriptor.as_mut() {
            if !d.voice_prefix.contains("Zombie") {
                d.voice_prefix = if d.voice_prefix == "VoiceFemale" {
                    "FemaleZombie"
                } else {
                    "MaleZombie"
                }
                .to_string();
            }
        }
        let mut sprite_name = if character.female { "KateZ" } else { "BobZ" }.to_string();
        if palette != 1 {
            sprite_name.push_str(&palette.to_string());
        }
        let mut z = IsoZombie {
            character,
            palette,
            sprite_name,
            tint: [0.0; 3],
            zombie_id: 0,
            hurt_sound: String::new(),
            speed_type: -1,
            strength: -1,
            cognition: -1,
            memory: -1,
            sight: -1,
            hearing: -1,
            walk_type: None,
            walk_variant: "ZombieWalk".to_string(),
            speed_mod: 1.0,
            lunger: false,
            running: false,
            crawling: false,
            inactive: false,
            can_crawl_under_vehicle: true,
            current_state: None,
            idle_tick_count: 0,
            time_since_seen_flesh: 100000.0,
            time_since_respond_to_sound: 1000000.0,
            check_for_corpse_timer: 10000.0,
            allow_repath_delay: 0.0,
            bonus_spot_time: 0.0,
            movex: 0.0,
            movey: 0.0,
            on_floor: false,
            path: PathFindBehavior2::default(),
            anim: None,
            sp: SpState {
                enabled: world.sp.is_some(),
                ..SpState::default()
            },
        };
        if z.sp.enabled {
            // `this.setVariable("bGetUpFromKnees", false)`, `("bGetUpFromProne", false)`: plain slots
            let sp = &mut z.sp;
            sp.plain
                .set_variable_bool(&mut sp.pool, "bGetUpFromKnees", false);
            sp.plain
                .set_variable_bool(&mut sp.pool, "bGetUpFromProne", false);
        }
        z.init_sprite_parts_zombie(world);
        for t in &mut z.tint {
            *t = 0.95 + world.rand.next(5) as f32 / 100.0;
        }
        // setDefaultState, setFakeDead(false): no draws
        z.inactive = zombie_inactivity_phase(&world.lore)?;
        z.do_zombie_stats(world)?;
        z.character.moving.width = 0.3;
        let voice = z
            .character
            .descriptor
            .as_ref()
            .map_or("", |d| d.voice_prefix.as_str());
        z.hurt_sound = format!("{voice}Hurt");
        // initializeStates(), `getActionContext().setGroup(ActionGroup.getActionGroup("zombie"))`; the animator's
        // `init(owner, GetAnimSetName())` ran in the constructor of IsoGameCharacter (not crawling yet: "zombie")
        if let Some(assets) = world.anim_assets.clone() {
            let mut action_context = ActionContext::new();
            action_context.set_group(assets.action_group);
            z.anim = Some(ZombieAnim {
                action_context,
                animator: {
                    let mut animator = AdvancedAnimator::init(Some(assets.anim_set), assets.clips);
                    animator.motion_scale = assets.motion_scale;
                    animator.rotation_scale = assets.rotation_scale;
                    animator
                },
                game_variables: AnimationVariableSource::default(),
                animation_invisible_frame_delay: FrameDelay::new(4),
                animation_time_scale: 1.0,
                animation_updating_this_frame: false,
            });
        }
        Ok(z)
    }

    /// zombie.characters.IsoZombie.InitSpritePartsZombie: the legs sprite takes the descriptor's torso name (not
    /// modelled), `zombieId = Rand.Next(10000)`.
    fn init_sprite_parts_zombie(&mut self, world: &mut ZombieWorld) {
        self.zombie_id = world.rand.next(10000);
    }

    /// zombie.characters.IsoZombie.DoZombieStats
    pub fn do_zombie_stats(&mut self, world: &mut ZombieWorld) -> Result<(), String> {
        let lore = world.lore;
        let rand = &mut world.rand;
        if lore.cognition == 1 {
            self.cognition = 1;
        }
        if lore.cognition == 4 {
            if rand.next(100) < lore.door_opening_percentage {
                self.cognition = 1;
            } else {
                self.cognition = 0;
            }
        }
        if self.strength == -1 && lore.strength == 1 {
            self.strength = 5;
        }
        if self.strength == -1 && lore.strength == 2 {
            self.strength = 3;
        }
        if self.strength == -1 && lore.strength == 3 {
            self.strength = 1;
        }
        if self.strength == -1 && lore.strength == 4 {
            self.strength = rand.next_range(1, 5);
        }
        let memory_setting = lore.memory;
        let mut random_memory = -1;
        if memory_setting == 5 {
            random_memory = rand.next(4);
        } else if memory_setting == 6 {
            random_memory = rand.next(3) + 1;
        }
        if self.memory == -1 && memory_setting == 1 || random_memory == 0 {
            self.memory = 1250;
        }
        if self.memory == -1 && memory_setting == 2 || random_memory == 1 {
            self.memory = 800;
        }
        if self.memory == -1 && memory_setting == 3 || random_memory == 2 {
            self.memory = 500;
        }
        if self.memory == -1 && memory_setting == 4 || random_memory == 3 {
            self.memory = 25;
        }
        if lore.sight == 4 {
            self.sight = rand.next(3) + 1;
        } else if lore.sight == 5 {
            self.sight = rand.next(2) + 2;
        } else {
            self.sight = lore.sight;
        }
        if lore.hearing == 4 {
            self.hearing = rand.next(3) + 1;
        } else if lore.hearing == 5 {
            self.hearing = rand.next(2) + 2;
        } else {
            self.hearing = lore.hearing;
        }
        self.do_zombie_speed(world);
        self.init_can_crawl_under_vehicle(world);
        Ok(())
    }

    /// zombie.characters.IsoZombie.doZombieSpeed()
    pub fn do_zombie_speed(&mut self, world: &mut ZombieWorld) {
        self.do_zombie_speed_internal(world, self.speed_type);
    }

    /// zombie.characters.IsoZombie.doZombieSpeedInternal
    fn do_zombie_speed_internal(&mut self, world: &mut ZombieWorld, zombie_speed: i32) {
        let zombie_speed = self.determine_zombie_speed(world, zombie_speed);
        let lore_speed = world.lore.speed;
        if self.crawling {
            self.do_crawler_speed(world, zombie_speed);
        } else if lore_speed == 3 || zombie_speed == 3 {
            self.do_shambler(world);
        } else if world.rand.next(3) != 0 {
            self.do_fake_shambler(world, zombie_speed);
        } else if lore_speed != 2 && zombie_speed != 2 {
            if lore_speed == 1 || zombie_speed == 1 {
                self.do_sprinter(world);
            }
        } else {
            self.do_fast_shambler(world);
        }
    }

    /// zombie.characters.IsoZombie.determineZombieSpeed
    fn determine_zombie_speed(&self, world: &mut ZombieWorld, zombie_speed: i32) -> i32 {
        if zombie_speed != -1 {
            return zombie_speed;
        }
        let mut zombie_speed = world.lore.speed;
        if self.inactive {
            zombie_speed = 3;
        } else if zombie_speed == 4 {
            if world.rand.next(100) < world.lore.sprinter_percentage {
                zombie_speed = 1;
            } else {
                zombie_speed = world.rand.next(2) + 2;
            }
        }
        zombie_speed
    }

    /// `this.speedMod = base; this.speedMod = this.speedMod + Rand.Next(1500) / 10000.0F;`
    fn set_speed_mod(&mut self, world: &mut ZombieWorld, base: f32) {
        self.speed_mod = base;
        self.speed_mod += world.rand.next(1500) as f32 / 10000.0;
    }

    /// zombie.characters.IsoZombie.doCrawlerSpeed (`def.animFrameIncrease *= 0.8F` not modelled)
    fn do_crawler_speed(&mut self, world: &mut ZombieWorld, zombie_speed: i32) {
        self.set_speed_mod(world, 0.3);
        self.do_zombie_speed_internal2(world, zombie_speed);
    }

    /// zombie.characters.IsoZombie.doSprinter (the sprite instance's frame speed is not modelled)
    fn do_sprinter(&mut self, world: &mut ZombieWorld) {
        self.lunger = true;
        self.walk_variant = "ZombieWalk2".to_string();
        self.set_speed_mod(world, 0.85);
        self.speed_type = 1;
        self.do_zombie_speed_internal2(world, self.speed_type);
    }

    /// zombie.characters.IsoZombie.doFastShambler
    fn do_fast_shambler(&mut self, world: &mut ZombieWorld) {
        self.lunger = true;
        self.walk_variant = "ZombieWalk2".to_string();
        self.set_speed_mod(world, 0.85);
        self.speed_type = 2;
        self.do_zombie_speed_internal2(world, self.speed_type);
    }

    /// zombie.characters.IsoZombie.doFakeShambler(int)
    fn do_fake_shambler(&mut self, world: &mut ZombieWorld, zombie_speed: i32) {
        self.set_speed_mod(world, 0.55);
        self.walk_variant = "ZombieWalk1".to_string();
        self.do_zombie_speed_internal2(world, zombie_speed);
    }

    /// zombie.characters.IsoZombie.doShambler
    fn do_shambler(&mut self, world: &mut ZombieWorld) {
        self.set_speed_mod(world, 0.55);
        self.walk_variant = "ZombieWalk1".to_string();
        self.speed_type = 3;
        self.do_zombie_speed_internal2(world, self.speed_type);
    }

    /// zombie.characters.IsoZombie.doZombieSpeedInternal2(int): the walk type, `slow1..3`, `1..5` or `sprint1..5`.
    fn do_zombie_speed_internal2(&mut self, world: &mut ZombieWorld, zombie_speed: i32) {
        let mut walk_type = if zombie_speed == 3 {
            format!("slow{}", world.rand.next(3) + 1)
        } else {
            (world.rand.next(5) + 1).to_string()
        };
        if zombie_speed == 1 {
            self.character.turn_delta_normal = 1.0;
            walk_type = format!("sprint{walk_type}");
        }
        self.walk_type = Some(walk_type);
        self.speed_type = zombie_speed;
    }

    /// zombie.characters.IsoZombie.initCanCrawlUnderVehicle: `Rand.Next(100) < chance` for the option's chance.
    fn init_can_crawl_under_vehicle(&mut self, world: &mut ZombieWorld) {
        let chance = match world.lore.crawl_under_vehicle {
            1 => 0,
            2 => 5,
            3 => 10,
            4 => 25,
            5 => 50,
            6 => 75,
            _ => 100,
        };
        self.can_crawl_under_vehicle = world.rand.next(100) < chance;
    }
}

impl IsoZombie {
    /// zombie.iso.IsoMovingObject.setX/setY/setZ of a zombie put on a square (the square's own moving-object list and
    /// the cell's object list are the caller's).
    pub fn place(&mut self, x: f32, y: f32, z: f32) {
        self.character.moving.x = x;
        self.character.moving.y = y;
        self.character.moving.z = z;
        // setX/setY also set `nx`/`ny`; setZ sets `lz`
        self.sp.next_x = x;
        self.sp.next_y = y;
        self.sp.last_z = z;
    }

    /// One `preupdate(); update(); postupdate();` of the zombie in the world's update loop, for a zombie with no
    /// target standing on a loaded square, alone on it, in a dedicated server's world with no player, no sound and no
    /// corpse. The order is the measured one: `update` first (the state machine has no state on the first tick), the
    /// action system's first state entry in `postupdate`.
    ///
    /// `squares` answers `IsoCell.getGridSquare`.
    pub fn tick(&mut self, world: &mut ZombieWorld, squares: &dyn GetSquare) -> Result<(), String> {
        self.preupdate(world)?;
        self.update(world, squares)?;
        self.postupdate(world)
    }

    /// zombie.characters.IsoZombie.preupdate -> IsoGameCharacter.preupdate: `updateAnimationTimeDelta()`. (Network
    /// bookkeeping and the debug variables are not modelled.)
    pub fn preupdate(&mut self, world: &ZombieWorld) -> Result<(), String> {
        let anim = self
            .anim
            .as_mut()
            .ok_or("IsoZombie.preupdate: the zombie was made without the animation assets")?;
        // updateAnimationTimeDelta: no player exists, so `maxAlpha` stays 0 and the zombie's animation is updated every
        // (delay + 1)-th frame, at that many frames' time. (IsoZombie.allowsInvisibleAnimationSkips: not a remote
        // zombie, `perObjectMultiplier` 1, `zombieAnimationDelay` debug option default true.)
        // IsoMovingObject.preupdate: `setNextX(getX()); setNextY(getY())`
        self.sp.next_x = self.character.moving.x;
        self.sp.next_y = self.character.moving.y;
        anim.animation_time_scale = 1.0;
        anim.animation_updating_this_frame = true;
        let max_alpha = 0.0f32;
        if max_alpha < 0.03 {
            anim.animation_updating_this_frame = anim.animation_invisible_frame_delay.update(
                world.time_delta(),
                world.multiplier,
                1.0,
            );
            if anim.animation_updating_this_frame {
                anim.animation_time_scale = (anim.animation_invisible_frame_delay.delay + 1) as f32;
            }
        }
        Ok(())
    }

    /// zombie.characters.IsoZombie.update -> updateInternal, the path of an idle zombie without a target. `Err` where
    /// the original would take a path this port does not have (corpse search, sheet rope, inactivity phase).
    ///
    /// NOT PORTED here (UNKNOWN beyond what the oracle shows for one zombie in an empty world): `ensureOnTile`,
    /// `tryThump(null)`, `separate` and `ZombieGroupManager.update` (a lone zombie), `RespondToSound` (a no-op under
    /// `GameServer.server`), `OnZombieUpdate` Lua handlers, `checkZombieEntersPlayerBuilding`, the farming-plant and
    /// campfire checks of the square, the emitter, `doDeferredMovement` and `updateMovementStatistics`.
    pub fn update(
        &mut self,
        world: &mut ZombieWorld,
        squares: &dyn GetSquare,
    ) -> Result<(), String> {
        let mult = world.multiplier;
        let thirty = world.thirty_fps_multiplier();
        // updateActiveState: makeInactive(isZombieInactivityPhase())
        if zombie_inactivity_phase(&world.lore)? != self.inactive {
            return Err(
                "IsoZombie.makeInactive: the zombie's activity phase changed (not ported)".into(),
            );
        }
        // `this.bonusSpotTime = PZMath.clamp(this.bonusSpotTime - GameTime.instance.getMultiplier(), 0.0F, Float.MAX_VALUE)`
        self.bonus_spot_time = pz_math::clamp(self.bonus_spot_time - mult, 0.0, f32::MAX);
        self.time_since_seen_flesh =
            pz_math::clamp(self.time_since_seen_flesh + mult, 0.0, f32::MAX);
        // super.update() = IsoGameCharacter.updateInternal: the state timer, then StateMachine.update
        self.character.moving.state_event_delay_timer -= thirty;
        if self.current_state == Some(ZombieState::Idle) {
            self.idle_execute(world, squares);
        }
        // the zombie is in ZombieIdleState (or has none yet): not one of the states that skip the rest
        if self.current_state.is_some() && self.current_state != Some(ZombieState::Idle) {
            return Err("IsoZombie.update: a state other than idle is not ported".into());
        }
        self.damage_sheet_rope(world, squares)?;
        self.allow_repath_delay = pz_math::clamp(self.allow_repath_delay - mult, 0.0, f32::MAX);
        self.time_since_respond_to_sound += thirty;
        self.update_search_for_corpse(thirty)
    }

    /// zombie.characters.IsoZombie.damageSheetRope: `Rand.Next(30) == 0` then the square's climb-sheet flags.
    pub(crate) fn damage_sheet_rope(
        &mut self,
        world: &mut ZombieWorld,
        squares: &dyn GetSquare,
    ) -> Result<(), String> {
        if world.rand.next(30) == 0 {
            let m = &self.character.moving;
            let sq = squares.get_grid_square(
                pz_math::fastfloor(m.x),
                pz_math::fastfloor(m.y),
                pz_math::fastfloor(m.z),
            );
            if let Some(sq) = sq {
                if [
                    IsoFlagType::climbSheetN,
                    IsoFlagType::climbSheetE,
                    IsoFlagType::climbSheetS,
                    IsoFlagType::climbSheetW,
                ]
                .iter()
                .any(|f| sq.has(*f))
                {
                    return Err(
                        "IsoZombie.damageSheetRope: a sheet rope on the square (not ported)".into(),
                    );
                }
            }
        }
        Ok(())
    }

    /// zombie.characters.IsoZombie.updateSearchForCorpse for a zombie with no target, no body to eat and no dead body
    /// anywhere (`!crawling && target == null && eatBodyTarget == null`, `bodyToEat == null`): the timer counts down by
    /// `getThirtyFPSMultiplier()`; when it runs out the original scans 20 x 20 squares for a corpse, which is not ported.
    fn update_search_for_corpse(&mut self, thirty: f32) -> Result<(), String> {
        if self.crawling {
            self.check_for_corpse_timer = 10000.0;
            return Ok(());
        }
        self.check_for_corpse_timer -= thirty;
        if self.check_for_corpse_timer <= 0.0 {
            return Err("IsoZombie.updateSearchForCorpse: the corpse scan is not ported".into());
        }
        Ok(())
    }

    /// zombie.characters.IsoZombie.postupdate -> IsoGameCharacter.postupdate -> `postUpdateAnimating()` on a dedicated
    /// server (`GameServer.server ||`): the action context updates, its state-changed callbacks run (the animator's
    /// `setState`, the AI state machine's `changeState`), then the animator and the animation player's tracks
    /// advance by `getAnimationTimeDelta()`.
    ///
    /// NOT PORTED (UNKNOWN): `animPlayer.updateForwardDirection/updateVerticalAimAngle`, the turning variables,
    /// `applyDeltas`, the bone and deferred-movement part of `AnimationPlayer.Update`, and what follows in
    /// `IsoZombie.postUpdateInternal` (unbalanced level, dragged corpse bleeding, the removal of a zombie off the map).
    pub fn postupdate(&mut self, world: &mut ZombieWorld) -> Result<(), String> {
        let anim = self
            .anim
            .as_mut()
            .ok_or("IsoZombie.postupdate: the zombie was made without the animation assets")?;
        let changed = anim.action_context.update(&anim.game_variables);
        if changed {
            self.action_state_changed(world)?;
        }
        let dt = world.time_delta() * self.anim_ref()?.animation_time_scale;
        // `if (this.getCurrentSquare() != null)`: the zombie stands on a square
        let anim = self.anim_mut()?;
        {
            let mut owner = ZombieOwner {
                ctx: &mut anim.action_context,
                vars: &anim.game_variables,
            };
            let mut env = Env {
                owner: &mut owner,
                rand: &mut world.rand,
            };
            anim.animator.update(dt, &mut env)?;
        }
        for e in [
            "ActiveAnimFinished",
            "ActiveAnimFinishing",
            "ActiveAnimLooped",
        ] {
            anim.action_context.clear_event(e);
        }
        // applyDeltas: not ported; then updateAnimPlayer -> AnimationPlayer.Update(getAnimationTimeDelta())
        {
            let mut owner = ZombieOwner {
                ctx: &mut anim.action_context,
                vars: &anim.game_variables,
            };
            let mut env = Env {
                owner: &mut owner,
                rand: &mut world.rand,
            };
            anim.animator.update_animation_player(dt, &mut env)?;
        }
        Ok(())
    }

    fn anim_ref(&self) -> Result<&ZombieAnim, String> {
        self.anim
            .as_ref()
            .ok_or_else(|| "the zombie was made without the animation assets".to_string())
    }

    fn anim_mut(&mut self) -> Result<&mut ZombieAnim, String> {
        self.anim
            .as_mut()
            .ok_or_else(|| "the zombie was made without the animation assets".to_string())
    }

    /// zombie.characters.component.StateMachineComponent.actionStateChanged (the callback the zombie's
    /// `ActionContext` runs when its states changed): `AdvancedAnimator.setState(root, children)`, then
    /// `StateMachine.changeState(tryGetAIState(root), children's AI states, false)`. `IsoZombie.actionStateChanged`
    /// and `ActionContext.invokeAnyStateChangedEvents` also call `NetworkZombieAI.extraUpdate()` on a server: network
    /// state, not ported (UNKNOWN).
    fn action_state_changed(&mut self, world: &mut ZombieWorld) -> Result<(), String> {
        let anim = self.anim_mut()?;
        let root = anim.action_context.get_current_state_name();
        let children = anim.action_context.get_child_state_names();
        anim.animator.set_state_names(&root, &children)?;
        // registerAIState: only "idle" (ZombieIdleState) is ported
        if !java::equals_ignore_case(&root, "idle") || !children.is_empty() {
            return Err(format!(
                "IsoZombie.actionStateChanged: the AI state of action state '{root}' (sub states {children:?}) is not ported"
            ));
        }
        // StateMachine.changeRootState: a new state is entered (`OnAIStateChange` Lua event: no handler modelled)
        if self.current_state != Some(ZombieState::Idle) {
            if self.current_state.is_some() {
                return Err("StateMachine.changeState: leaving a state is not ported".into());
            }
            self.current_state = Some(ZombieState::Idle);
            self.idle_enter(world);
        }
        Ok(())
    }

    /// zombie.ai.states.ZombieIdleState.enter
    pub(crate) fn idle_enter(&mut self, world: &mut ZombieWorld) {
        self.movex = 0.0;
        self.movey = 0.0;
        self.character.moving.state_event_delay_timer = pick_random_wander_interval(world);
        self.idle_tick_count = 0;
    }

    /// zombie.ai.states.ZombieIdleState.execute, without `Core.lastStand`, a useless or indoor zombie.
    fn idle_execute(&mut self, world: &mut ZombieWorld, squares: &dyn GetSquare) {
        // the server's world has no player and no variables: `pathToAux` changes nothing visible there
        let _ = self.idle_execute_in(world, squares, None);
    }

    /// ZombieIdleState.execute of the single-player zombie: as the server's, and `pathToLocation` goes on to
    /// `pathToAux`, which sets `bPathfind` and `bMoving` from the line to the goal.
    pub(crate) fn idle_execute_sp(
        &mut self,
        world: &mut ZombieWorld,
        rd: &super::iso_zombie_sp::SpRead,
    ) -> Result<(), String> {
        self.idle_execute_in(world, rd.squares, Some(rd))
    }

    fn idle_execute_in(
        &mut self,
        world: &mut ZombieWorld,
        squares: &dyn GetSquare,
        rd: Option<&super::iso_zombie_sp::SpRead>,
    ) -> Result<(), String> {
        self.movex = 0.0;
        self.movey = 0.0;
        // `if (!zombie.isReanimatedForGrappleOnly()) zombie.setOnFloor(zombie.crawling)`
        self.on_floor = self.crawling;
        // (tickCount == 2 sets the audio parameter ParameterZombieState.Idle: not modelled)
        self.idle_tick_count += 1;
        if self.character.moving.state_event_delay_timer <= 0.0 {
            self.character.moving.state_event_delay_timer = pick_random_wander_interval(world);
            let (px, py, pz) = (
                self.character.moving.x,
                self.character.moving.y,
                self.character.moving.z,
            );
            let (sx, sy, sz) = (
                pz_math::fastfloor(px),
                pz_math::fastfloor(py),
                pz_math::fastfloor(pz),
            );
            let x = sx + world.rand.next(8) - 4;
            let y = sy + world.rand.next(8) - 4;
            if let Some(sq) = squares.get_grid_square(x, y, sz) {
                // the zombie stands in its own square's movingObjects (and the player in his)
                let own = x == sx && y == sy;
                let player_here = rd.is_some_and(|rd| {
                    pz_math::fastfloor(rd.world.player.x) == x
                        && pz_math::fastfloor(rd.world.player.y) == y
                        && pz_math::fastfloor(rd.world.player.z) == sz
                });
                if sq.is_free(true, own || player_here) {
                    // IsoGameCharacter.pathToLocation: PathFindBehavior2.pathToLocation, then pathToAux
                    self.path.path_to_location((px, py, pz), x, y, sz);
                    if let Some(rd) = rd {
                        self.path_to_aux(x as f32 + 0.5f32, y as f32 + 0.5f32, sz as f32, rd)?;
                    }
                    self.allow_repath_delay = 200.0;
                }
            }
        }
        Ok(())
    }
}

/// zombie.ai.states.ZombieIdleState.pickRandomWanderInterval: `Rand.Next(400, 1000)`, times 1.5 unless it is raining.
fn pick_random_wander_interval(world: &mut ZombieWorld) -> f32 {
    let mut interval = world.rand.next_range(400, 1000) as f32;
    if !world.raining {
        interval *= 1.5;
    }
    interval
}

/// zombie.GameTime.isZombieInactivityPhase: `!isZombieActivityPhase()`. With `ActiveOnly` "both" (1) the zombies are
/// always active; the day and night options need the climate's dawn and dusk, which this port does not feed in yet.
pub(crate) fn zombie_inactivity_phase(lore: &Lore) -> Result<bool, String> {
    if lore.active_only == 1 {
        Ok(false)
    } else {
        Err("GameTime.isZombieInactivityPhase with ActiveOnly other than 1 needs the season's dawn and dusk (not ported here)".into())
    }
}
