//! zombie.characters.IsoZombie in the single-player game (`GameServer.server` false), as far as a zombie with a target
//! or a wander goal needs it: the variables `registerVariableCallbacks` registers (read lazily, as the original's
//! callback slots are), `updateInternal`, `postUpdateInternal`/`postUpdateAnimating`, `StateMachine.changeState` and the
//! AI states in `zombie::ai::states`.
//!
//! How the original runs headless (measured, oracle `zombieai`): the dedicated-server harness with `GameServer.server`
//! set false before the zombie is built, a player stand-in (an `IsoPlayer` the camera follows) and an `IsoChunkMap`
//! over the loaded chunks, then `preupdate(); update(); postupdate()` per tick.
//!
//! NOT PORTED (UNKNOWN, each carried as an `Err` or a labelled assumption): the deferred (root) movement of the
//! animation tracks and so any walking (`doDeferredMovement`), `separate`, `ensureOnTile`, `RespondToSound` (assumed:
//! no world sound exists), `ZombieGroupManager.update` (a lone zombie), the Lua event `OnZombieUpdate`, line of
//! sight (`canSeeTarget` stays false: the squares' `couldSee` is never computed headless), vehicles, hits.
use std::cell::RefCell;
use std::rc::Rc;

use super::action::action_context::ActionContext;
use super::iso_zombie::{IsoZombie, ZombieAnim, ZombieState, ZombieWorld, ANIM_EVENT_HANDLERS};
use crate::java::{self, Random};
use crate::zombie::core::math::pz_math;
use crate::zombie::core::random::rand_abstract::RandAbstract;
use crate::zombie::core::skinnedmodel::advancedanimation::advanced_animator::{AnimatorOwner, Env};
use crate::zombie::core::skinnedmodel::advancedanimation::anim_event::{
    AnimEvent, AnimEventKind, AnimEventTime,
};
use crate::zombie::core::skinnedmodel::advancedanimation::animation_variable_reference::AnimationVariableReference;
use crate::zombie::core::skinnedmodel::advancedanimation::animation_variable_source::{
    AnimationVariableSlot, SlotValue,
};
use crate::zombie::core::skinnedmodel::advancedanimation::i_animation_variable_source::IAnimationVariableSource;
use crate::zombie::core::skinnedmodel::animation::animation_player::{
    vector2_get_direction, AnimationPlayer,
};
use crate::zombie::core::skinnedmodel::animation::animation_track::AnimationTrack;
use crate::zombie::core::tile_property_alias_map::TilePropertyAliasMap;
use crate::zombie::game_sounds::GameSounds;
use crate::zombie::iso::iso_grid_square::{GetSquare, IsoGridSquare};
use crate::zombie::iso::sprite::iso_sprite_manager::IsoSpriteManager;
use crate::zombie::iso::vector2::Vector2;
use crate::zombie::util::string_utils;

/// java.lang.Boolean.parseBoolean: true for the text "true" in any case, false for null
fn parse_boolean(v: Option<&str>) -> bool {
    v.is_some_and(|t| java::equals_ignore_case(t, "true"))
}

/// zombie.characters.IsoPlayer, as much as a zombie reads of the player it targets.
#[derive(Clone, Debug)]
pub struct PlayerStandIn {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// `getWidth()`: the `IsoMovingObject` default (0.24), measured through `distancetotarget`
    pub width: f32,
    /// `getHealth()` (nothing in the ported states changes it: the attack's bite is not ported)
    pub health: f32,
    /// `isCriticalHit()`: the wielder's critical hit flag a melee hit reads (set before the hit)
    pub critical_hit: bool,
}

/// The single-player world a zombie lives in: the one player, the chunks of its `IsoChunkMap` (chunk coordinates,
/// `x / 8`, `y / 8`) and the tile property aliases the squares' floor and slope tests need.
#[derive(Clone, Debug)]
pub struct SinglePlayerWorld {
    pub player: PlayerStandIn,
    pub loaded_chunks: Vec<(i32, i32)>,
    pub alias: Rc<TilePropertyAliasMap>,
    /// the loaded tile sprites (`IsoObject.isBush` reads the sprite's tileset name and properties)
    pub sprites: Rc<IsoSpriteManager>,
    /// the zombie's `IsoMovingObject.getID()`: the scenario's second moving object (the player stand-in is the first)
    pub zombie_object_id: i32,
    /// `GameSounds` after `ScriptManager.Load`: the sounds a `PlaySound` animation event picks a clip of (`None`: the
    /// sound scripts are not loaded and such an event is an `Err`)
    pub game_sounds: Option<Rc<GameSounds>>,
    /// `SandboxOptions.instance.bloodLevel.getValue()` (1..5, default 3): the blood and giblets of a kill
    pub blood_level: i32,
    /// The corpses `IsoDeadBody(died)` made, in order (the original puts them on the square's static moving objects)
    pub corpses: Vec<super::iso_zombie_kill::Corpse>,
}

impl SinglePlayerWorld {
    /// `IsoWorld.instance.currentCell.getChunkForGridSquare(x, y, z) != null`
    pub fn has_chunk_for_square(&self, x: i32, y: i32) -> bool {
        self.loaded_chunks
            .contains(&(x.div_euclid(8), y.div_euclid(8)))
    }
}

/// A `GetSquare` over a `&dyn GetSquare` (the squares' generic helpers need a sized type).
pub struct Sq<'a>(pub &'a dyn GetSquare);

impl GetSquare for Sq<'_> {
    fn get_grid_square(&self, x: i32, y: i32, z: i32) -> Option<&IsoGridSquare> {
        self.0.get_grid_square(x, y, z)
    }
}

/// The 164 variables the single-player zombie has at construction (`registerVariableCallbacks` of `IsoGameCharacter`,
/// `BaseGrappleable` and `IsoZombie`, in registration order): name in lower case, slot type and the value read from the
/// original (oracle `zombieai`, the `VAR` lines; "?" is a variable whose getter makes the animation player, which the
/// first animation frame does in the game, so it is not read).
pub const REGISTERED: [(&str, &str, &str); 164] = [
    ("isrendered", "Boolean", "true"),
    ("stateeventdelaytimer", "Float", "0.0"),
    ("hashitreaction", "Boolean", "false"),
    ("hitreaction", "String", ""),
    ("collidetype", "String", "null"),
    ("footinjurytype", "String", ""),
    ("bumptype", "String", ""),
    ("onbed", "Boolean", "false"),
    ("sittingonfurniture", "Boolean", "false"),
    ("sitonground", "Boolean", "false"),
    ("nearwallcrouching", "Boolean", "false"),
    ("canclimbdownrope", "Boolean", "false"),
    ("frombehind", "Boolean", "false"),
    ("fallonfront", "Boolean", "false"),
    ("killedbyfall", "Boolean", "false"),
    ("intrees", "Boolean", "false"),
    ("bumped", "Boolean", "false"),
    ("bumpdone", "Boolean", "false"),
    ("bumpfall", "Boolean", "false"),
    ("bumpfalltype", "String", ""),
    ("bumpstaggered", "Boolean", "false"),
    ("bonfloor", "Boolean", "false"),
    ("isprone", "Boolean", "false"),
    ("isgettingup", "Boolean", "false"),
    ("rangedweaponempty", "Boolean", "false"),
    ("footinjury", "Boolean", "false"),
    ("choptreespeed", "Float", "0.8"),
    ("movedelta", "Float", "1.0"),
    ("turndelta", "Float", "1.0"),
    ("angle", "Float", "-45.000004"),
    ("animangle", "Float", "-45.000004"),
    ("twist", "Float", "0.0"),
    ("targettwist", "Float", "0.0"),
    ("maxtwist", "Float", "15.0"),
    ("shouldertwist", "Float", "0.0"),
    ("excesstwist", "Float", "0.0"),
    ("numtwistbones", "Int", "?"),
    ("anglestepdelta", "Float", "0.0"),
    ("angletwistdelta", "Float", "0.0"),
    ("isturning", "Boolean", "false"),
    ("isturning90", "Boolean", "false"),
    ("isturningaround", "Boolean", "false"),
    ("bmoving", "Boolean", "false"),
    ("beenmovingfor", "Float", "0.0"),
    ("previousstate", "String", "idle"),
    ("momentumscalar", "Float", "0.0"),
    ("hastimedactions", "Boolean", "false"),
    ("isoverencumbered", "Boolean", "false"),
    ("criticalhit", "Boolean", "false"),
    ("bknockeddown", "Boolean", "false"),
    ("bfalling", "Boolean", "false"),
    ("bdead", "Boolean", "false"),
    ("falltime", "Float", "0.0"),
    ("fallspeedseverity", "String", "None"),
    ("bgetupfromknees", "Boolean", "false"),
    ("bgetupfromprone", "Boolean", "false"),
    ("aim", "Boolean", "false"),
    ("baimatfloor", "Boolean", "false"),
    ("aimatflooramount", "Float", "0.0"),
    ("verticalaimangle", "Float", "0.0"),
    ("attackanim", "Boolean", "false"),
    ("shoveanim", "Boolean", "false"),
    ("stompanim", "Boolean", "false"),
    ("isstompanim", "Boolean", "false"),
    ("performinghostileanim", "Boolean", "false"),
    ("firemode", "String", ""),
    ("shouttype", "String", "null"),
    ("shoutitemmodel", "String", "null"),
    ("isanimatingbackwards", "Boolean", "false"),
    ("bdograpple", "Boolean", "false"),
    ("bdocontinuegrapple", "Boolean", "false"),
    ("bisgrappling", "Boolean", "false"),
    ("grappleresult", "String", ""),
    ("sharedgrappletype", "String", ""),
    ("sharedgrappleanimnode", "String", ""),
    ("sharedgrappletime", "Float", "0.0"),
    ("sharedgrapplefraction", "Float", "0.0"),
    ("grappleoffsetforward", "Float", "0.0"),
    ("grappleoffsetbehaviour", "String", "NONE"),
    ("bearingtograppledtarget", "Float", "0.0"),
    ("bbeinggrappled", "Boolean", "false"),
    ("grappledby", "String", ""),
    ("grappledbytype", "String", "None"),
    ("grapplegrabanim", "Boolean", "false"),
    ("grappleanim", "Boolean", "?"),
    ("anygrappleanim", "Boolean", "?"),
    ("grapplethrowoutwindow", "Boolean", "false"),
    ("grapplethrowoverfence", "Boolean", "false"),
    ("grapplethrowintocontainer", "Boolean", "false"),
    ("canragdoll", "Boolean", "?"),
    ("iseditingragdoll", "Boolean", "false"),
    ("isragdoll", "Boolean", "false"),
    ("issimulationactive", "Boolean", "false"),
    ("isupright", "Boolean", "?"),
    ("isonback", "Boolean", "?"),
    ("isragdollfall", "Boolean", "false"),
    ("isvehiclecollision", "Boolean", "false"),
    ("usephysichitreaction", "Boolean", "false"),
    ("useragdollvehiclecollision", "Boolean", "?"),
    ("bheadlookaround", "Boolean", "false"),
    ("lookhorizontal", "Float", "0.0"),
    ("lookvertical", "Float", "0.0"),
    ("hitforce", "Float", "0.0"),
    ("hitdir", "String", "FRONT"),
    ("hitdir.x", "Float", "0.0"),
    ("hitdir.y", "Float", "0.0"),
    ("recoilvarx", "Float", "0.0"),
    ("recoilvary", "Float", "0.0"),
    ("hideequippedhandl", "Boolean", "false"),
    ("hideequippedhandr", "Boolean", "false"),
    ("aimingmode", "String", "NOT_AIMING"),
    ("hastarget", "Boolean", "false"),
    ("isunarmed", "Boolean", "true"),
    ("ismeleeweaponequipped", "Boolean", "false"),
    ("israngedweaponequipped", "Boolean", "false"),
    ("blandlight", "Boolean", "false"),
    ("blandlightmask", "Boolean", "false"),
    ("bhardfall", "Boolean", "false"),
    ("bhardfall2", "Boolean", "false"),
    ("fallimpactseverity", "String", "None"),
    ("bclient", "Boolean", "false"),
    ("bmultiplayer", "Boolean", "false"),
    ("bmovingnetwork", "Boolean", "true"),
    ("hitheadtype", "Int", "0"),
    ("realstate", "String", "idle"),
    ("bisreanimatedforgrappleonly", "Boolean", "false"),
    ("battack", "Boolean", "false"),
    ("isfacingtarget", "Boolean", "false"),
    ("targetseentime", "Float", "0.0"),
    ("battackvehicle", "Boolean", "false"),
    ("beatbodytarget", "Boolean", "false"),
    ("bbecomecrawler", "Boolean", "false"),
    ("bfakedead", "Boolean", "false"),
    ("bhastarget", "Boolean", "false"),
    ("bcanseetarget", "Boolean", "false"),
    ("shouldsprint", "Boolean", "false"),
    ("blunge", "Boolean", "false"),
    ("bstoplunging", "Boolean", "true"),
    ("bpassengerexposed", "Boolean", "false"),
    ("bistargetissmallvehicle", "Boolean", "true"),
    ("breanimate", "Boolean", "false"),
    ("bstaggerback", "Boolean", "false"),
    ("unbalancedlevel", "Float", "0.0"),
    ("bthump", "Boolean", "false"),
    ("bundervehicle", "Boolean", "false"),
    ("bbeingsteppedon", "Boolean", "false"),
    ("distancetotarget", "Float", "-1.0"),
    ("lasttargetseen", "Boolean", "false"),
    ("lungetimer", "Float", "0.0"),
    ("reanimatetimer", "Float", "0.0"),
    ("turndirection", "String", ""),
    ("alerted", "Boolean", "false"),
    ("zombiewalktype", "String", "2"),
    ("crawlertype", "Int", "0"),
    ("bgetupfromcrawl", "Boolean", "false"),
    ("playerattackposition", "String", "null"),
    ("eatspeed", "Float", "1.0"),
    ("issitting", "Boolean", "false"),
    ("bknifedeath", "Boolean", "false"),
    ("bjawstabattach", "Boolean", "false"),
    ("bpathfindprediction", "Boolean", "false"),
    ("bcrawling", "Boolean", "false"),
    ("attackdiddamage", "Boolean", "false"),
    ("attackoutcome", "String", ""),
];

/// The registered variables whose slot has a setter (`this.setVariable(name, getter, setter, ...)`); `setVariable` of
/// any other registered variable only warns and changes nothing.
const SETTABLE_VARIABLES: [&str; 52] = [
    "isrendered",
    "hitreaction",
    "collidetype",
    "bumptype",
    "onbed",
    "sittingonfurniture",
    "sitonground",
    "nearwallcrouching",
    "frombehind",
    "fallonfront",
    "killedbyfall",
    "bumpdone",
    "bumpfall",
    "bumpfalltype",
    "bumpstaggered",
    "bonfloor",
    "rangedweaponempty",
    "movedelta",
    "turndelta",
    "angle",
    "maxtwist",
    "isturning",
    "isturning90",
    "isturningaround",
    "bmoving",
    "momentumscalar",
    "criticalhit",
    "bknockeddown",
    "attackanim",
    "shoveanim",
    "stompanim",
    "isstompanim",
    "isanimatingbackwards",
    "grapplethrowoutwindow",
    "grapplethrowoverfence",
    "grapplethrowintocontainer",
    "iseditingragdoll",
    "isragdollfall",
    "isvehiclecollision",
    "usephysichitreaction",
    "recoilvarx",
    "recoilvary",
    "hideequippedhandl",
    "hideequippedhandr",
    "bbecomecrawler",
    "breanimate",
    "unbalancedlevel",
    "bknifedeath",
    "bjawstabattach",
    "bcrawling",
    "attackdiddamage",
    "attackoutcome",
];

/// The two registered variables that are plain slots (`this.setVariable("bGetUpFromKnees", false)`), not callbacks.
const PLAIN_REGISTERED: [&str; 2] = ["bgetupfromknees", "bgetupfromprone"];

/// The String variables whose getter returns null at construction.
const NULL_STRINGS: [&str; 4] = [
    "collidetype",
    "shouttype",
    "shoutitemmodel",
    "playerattackposition",
];

/// What a variable callback reads besides the zombie.
pub struct SpRead<'a> {
    pub squares: &'a dyn GetSquare,
    pub world: &'a SinglePlayerWorld,
    /// the first registered variable whose callback is not ported that was read (the tick then fails)
    pub unported: RefCell<Option<String>>,
}

impl<'a> SpRead<'a> {
    pub fn new(squares: &'a dyn GetSquare, world: &'a SinglePlayerWorld) -> Self {
        SpRead {
            squares,
            world,
            unported: RefCell::new(None),
        }
    }

    fn flag(&self, what: &str) {
        let mut u = self.unported.borrow_mut();
        if u.is_none() {
            *u = Some(what.to_string());
        }
    }

    /// `Err` when something not ported was read since the last check.
    pub fn check(&self) -> Result<(), String> {
        match self.unported.borrow_mut().take() {
            Some(n) => Err(format!("IsoZombie: {n} is not ported")),
            None => Ok(()),
        }
    }
}

fn slot(key: &str, value: SlotValue) -> AnimationVariableSlot {
    AnimationVariableSlot {
        key: key.to_string(),
        value,
        read_only: false,
    }
}

/// `(180.0F / (float) Math.PI) * radians`
fn to_degrees(radians: f32) -> f32 {
    (180.0f32 / PI) * radians
}

const PI: f32 = std::f64::consts::PI as f32;

/// zombie.core.math.PZMath.abs (float): `val * sign(val)`.
fn pz_abs(val: f32) -> f32 {
    val * pz_math::sign(val) as f32
}

/// zombie.iso.Vector2.dot
fn dot(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    x1 * x2 + y1 * y2
}

impl IsoZombie {
    /// The square at a position (`IsoCell.getGridSquare(floor x, floor y, floor z)`).
    pub(crate) fn sp_square(
        squares: &dyn GetSquare,
        x: f32,
        y: f32,
        z: f32,
    ) -> Option<&IsoGridSquare> {
        squares.get_grid_square(
            pz_math::fastfloor(x),
            pz_math::fastfloor(y),
            pz_math::fastfloor(z),
        )
    }

    /// IsoGameCharacter.getDirectionAngleRadians: `forwardDirection.getDirection()`.
    fn direction_angle_radians(&self) -> f32 {
        vector2_get_direction(
            self.character.forward_direction.x,
            self.character.forward_direction.y,
        )
    }

    /// IsoGameCharacter.getAnimAngleRadians: the animation player's `angle` once `isReady()` and its bone transforms have
    /// had their first frame, else `forwardDirection.getDirection()`. The headless player never gets that first frame
    /// (`boneTransformsNeedFirstFrame` stays true: the corpse angle of five kills equals the forward direction's angle
    /// while `getLookAngleRadians` reads the player's angle), so this is `getDirectionAngleRadians()`.
    pub(crate) fn anim_angle_radians(&self) -> f32 {
        self.direction_angle_radians()
    }

    /// IsoGameCharacter.getLookAngleRadians: the animation player's `angle + twistAngle` once it exists (headless it is
    /// ready: measured), else `getDirectionAngleRadians()`.
    pub fn look_angle_radians(&self) -> f32 {
        match &self.sp.anim_player {
            Some(p) => p.look_angle(),
            None => self.direction_angle_radians(),
        }
    }

    /// IsoGameCharacter.getLookVector: `setLengthAndDirection(getLookAngleRadians(), 1.0F)`.
    pub fn look_vector(&self) -> Vector2 {
        let a = f64::from(self.look_angle_radians());
        Vector2::new(
            (crate::java_fdlibm::cos(a) * 1.0) as f32,
            (crate::java_fdlibm::sin(a) * 1.0) as f32,
        )
    }

    /// IsoZombie.isFacingTarget
    pub fn is_facing_target(&self, rd: &SpRead) -> bool {
        if !self.sp.target {
            return false;
        }
        let mut tempo = Vector2::new(
            rd.world.player.x - self.character.moving.x,
            rd.world.player.y - self.character.moving.y,
        );
        tempo.normalize();
        if tempo.get_length() == 0.0 {
            true
        } else {
            let look = self.look_vector();
            dot(tempo.x, tempo.y, look.x, look.y) >= 0.8f32
        }
    }

    /// IsoZombie.isTargetLocationKnown
    pub fn is_target_location_known(&self) -> bool {
        if !self.sp.target {
            false
        } else if self.bonus_spot_time > 0.0 {
            true
        } else {
            self.time_since_seen_flesh < 1.0
        }
    }

    /// IsoZombie.getShouldAttack for a plain player on its feet (not in a vehicle, not a ghost, not on the floor) and a
    /// zombie that is not crawling, fake dead or reanimated.
    fn get_should_attack(&self, rd: &SpRead) -> bool {
        if !self.sp.target {
            return false;
        }
        let p = &rd.world.player;
        // `this.reanimate` false; `Math.abs(target.getZ() - getZ()) >= 0.2F`
        if (p.z - self.character.moving.z).abs() >= 0.2f32 {
            return false;
        }
        if self.crawling {
            rd.flag("battack of a crawling zombie");
            return false;
        }
        let m = &self.character.moving;
        let current = Self::sp_square(rd.squares, m.x, m.y, m.z);
        let target_sq = Self::sp_square(rd.squares, p.x, p.y, p.z);
        if let (Some(c), Some(t)) = (current, target_sq) {
            // `!BentFences.getInstance().isUnbentObject(this.target)`: a player is no bent fence object
            if c.is_something_to(t, &Sq(rd.squares)) {
                return false;
            }
        }
        let attack_range = 0.72f32;
        let len = self.sp.vector_to_target.get_length();
        len <= attack_range
    }

    /// IsoZombie.isSideOfStaircaseBetweenSelfAndTarget: only true with a staircase on either square (`flag`ged then).
    fn side_of_staircase(&self, rd: &SpRead) -> bool {
        if !self.sp.target {
            return false;
        }
        let m = &self.character.moving;
        let p = &rd.world.player;
        let c = Self::sp_square(rd.squares, m.x, m.y, m.z);
        let t = Self::sp_square(rd.squares, p.x, p.y, p.z);
        if let (Some(c), Some(t)) = (c, t) {
            if c.has_stairs() || t.has_stairs() {
                rd.flag("isSideOfStaircaseBetweenSelfAndTarget with stairs");
            }
        }
        false
    }

    /// The `blunge` callback of IsoZombie.registerVariableCallbacks for a plain player target.
    fn get_lunge(&self, rd: &SpRead) -> bool {
        if !self.sp.target {
            return false;
        }
        let m = &self.character.moving;
        let p = &rd.world.player;
        if pz_math::fastfloor(m.z) != pz_math::fastfloor(p.z) {
            return false;
        }
        // (no vehicle, no reanimated corpse, no ghost mode; `getThumpTarget() != null` is false, so the
        // `isSomethingTo` clause cannot return)
        if self.side_of_staircase(rd) {
            return false;
        }
        // `isCurrentState(ZombieTurnAlerted)`: not ported as a state, never current
        let len = self.sp.vector_to_target.get_length();
        if len > 3.5f32 {
            return false;
        }
        match crate::zombie::pathfind::line_clear_collide_main::is_not_clear_old(
            rd.squares,
            &rd.world.alias,
            m.x,
            m.y,
            p.x,
            p.y,
            pz_math::fastfloor(m.z),
            // lineClearCollide(..., ignoreDoors = false, closeToWalls = true)
            2,
        ) {
            Ok(collides) => !collides,
            Err(e) => {
                rd.flag(&format!("blunge ({e})"));
                false
            }
        }
    }

    /// The `bstoplunging` callback.
    fn get_stop_lunging(&self, rd: &SpRead) -> bool {
        if !self.sp.target {
            true
        } else if self.lunge_timer() > 0.0 {
            false
        } else {
            let distance_to_target = self.sp.vector_to_target.get_length()
                - self.character.moving.width
                + rd.world.player.width;
            if self.lunge_timer() <= 0.0 && distance_to_target >= 0.8f32 {
                true
            } else {
                self.side_of_staircase(rd)
            }
        }
    }

    fn lunge_timer(&self) -> f32 {
        self.sp.lunge_timer
    }

    /// The value of a registered variable that is computed from the zombie's state, `None` for a constant one.
    fn dynamic_value(&self, name: &str, rd: &SpRead) -> Option<SlotValue> {
        let b = |v: bool| Some(SlotValue::Bool(v));
        let f = |v: f32| Some(SlotValue::Float(v));
        let s = |v: &str| Some(SlotValue::String(Some(v.to_string())));
        let p = self.sp.anim_player.as_ref();
        match name {
            "stateeventdelaytimer" => f(self.character.moving.state_event_delay_timer),
            "bonfloor" => b(self.on_floor),
            "bmoving" => b(self.sp.is_moving),
            "bknockeddown" => b(self.sp.knocked_down),
            // IsoGameCharacter.hasHitReaction / getHitReaction, IsoZombie.isStaggerBack, getHitForce, getHitDir
            "hashitreaction" => b(!self.sp.hit_reaction.is_empty()),
            "hitreaction" => s(&self.sp.hit_reaction),
            "bstaggerback" => b(self.sp.stagger_back),
            "hitforce" => f(self.sp.hit_force),
            "hitdir.x" => f(self.sp.hit_dir.x),
            "hitdir.y" => f(self.sp.hit_dir.y),
            "fallonfront" => b(self.sp.fall_on_front),
            "bbecomecrawler" => b(self.sp.become_crawler),
            "reanimatetimer" => f(self.sp.reanimate_timer),
            "usephysichitreaction" => b(self.sp.use_physic_hit_reaction),
            "bbeingsteppedon" => match self.is_being_stepped_on(rd) {
                Ok(v) => b(v),
                Err(e) => {
                    rd.flag(&e);
                    None
                }
            },
            "bcrawling" => b(self.crawling),
            "bdead" => b(self.character.health <= 0.0),
            "alerted" => b(self.sp.alerted),
            "attackdiddamage" => b(self.sp.attack_did_damage),
            "attackoutcome" => s(&self.sp.attack_outcome),
            "angle" | "animangle" => f(to_degrees(self.direction_angle_radians())),
            "twist" => f(p.map_or(0.0, |p| to_degrees(p.twist_angle))),
            "targettwist" => f(p.map_or(0.0, |p| to_degrees(p.target_twist_angle))),
            "shouldertwist" => f(p.map_or(0.0, |p| to_degrees(p.shoulder_twist_angle))),
            "excesstwist" => f(p.map_or(0.0, |p| to_degrees(p.excess_twist))),
            "anglestepdelta" => f(p.map_or(0.0, |p| p.angle_step_delta)),
            "angletwistdelta" => f(p.map_or(0.0, |p| p.angle_twist_delta)),
            "maxtwist" => f(self.sp.max_twist),
            "movedelta" => f(self.sp.move_delta),
            "turndelta" => f(self.character.turn_delta_normal),
            "isturning" => b(self.sp.is_turning),
            "isturning90" => b(self.sp.is_turning90),
            "isturningaround" => b(self.sp.is_turning_around),
            "previousstate" => s(&self.sp.previous_action_state),
            "zombiewalktype" => Some(SlotValue::String(self.walk_type.clone())),
            "bhastarget" | "shouldsprint" => b(self.sp.target),
            "bcanseetarget" => b(self.sp.can_see_target),
            "battack" => b(self.get_should_attack(rd)),
            "isfacingtarget" => b(self.is_facing_target(rd)),
            "blunge" => b(self.get_lunge(rd)),
            "bstoplunging" => b(self.get_stop_lunging(rd)),
            "targetseentime" => f(self.sp.target_seen_time),
            "lasttargetseen" => b(self.sp.last_target_seen_x != -1),
            "lungetimer" => f(self.sp.lunge_timer),
            "distancetotarget" => f(if self.sp.target {
                self.sp.vector_to_target.get_length() - self.character.moving.width
                    + rd.world.player.width
            } else {
                -1.0
            }),
            "canragdoll"
            | "useragdollvehiclecollision"
            | "isupright"
            | "isonback"
            | "anygrappleanim"
            | "grappleanim"
            | "numtwistbones" => {
                if p.is_none() {
                    // the Java getters make the animation player (`getAnimationPlayer()`): not reproduced through a
                    // read-only view, the first animation update frame makes it before any state reads these
                    rd.flag(&format!("{name} (before the animation player exists)"));
                    return None;
                }
                match name {
                    // IsoGameCharacter.canRagdoll in the single-player game: no debug option, the default
                    // `usePhysicsHitReaction` option, no ragdoll controller (`getRagdollController()` null, 0 active
                    // simulations of 20), worn clothing allows it, `canCurrentStateRagdoll()` = `hasCurrentState()`
                    // (IStateFlagsSource.canRagdoll is true; only ClimbOverFenceState/ClimbThroughWindowState differ)
                    "canragdoll" => b(self.current_state.is_some()),
                    // `canRagdoll() && vehicleCollision`: no vehicle ever hits the zombie here
                    "useragdollvehiclecollision" => b(false),
                    // no ragdoll controller: not upright, not on the back
                    "isupright" | "isonback" => b(false),
                    // `isPerformingAnyGrappleAnimation` / `isPerformingGrappleAnimation`: the player is never ready
                    // headless (no model), its track list is empty
                    "anygrappleanim" | "grappleanim" => b(false),
                    // AnimationPlayer.getNumTwistBones: the five bones IsoGameCharacter.getAnimationPlayer sets
                    _ => f(5.0),
                }
            }
            "turndirection" => {
                // `getPath2() != null` is never true here; a target at a distance turns by the `calculateDir` of
                // CrawlingZombieTurnState unless it is already in the target's direction
                if self.sp.target && self.sp.vector_to_target.get_length() != 0.0 {
                    let v = self.sp.vector_to_target;
                    let target_dir =
                        crate::zombie::iso::iso_directions::IsoDirections::from_angle_xy(v.x, v.y);
                    if self.character.forward_iso_direction() == target_dir {
                        s("")
                    } else {
                        rd.flag("turndirection (CrawlingZombieTurnState.calculateDir)");
                        None
                    }
                } else {
                    s("")
                }
            }
            _ => None,
        }
    }

    /// IsoGameCharacter.getVariable(key) of the single-player zombie (without the action context's own variables): a
    /// registered callback, a plain slot or nothing. A registered callback that is not ported flags `rd` and gives
    /// `None`.
    pub fn sp_get_variable(&self, key: &str, rd: &SpRead) -> Option<AnimationVariableSlot> {
        let lower = java::case_insensitive_key(java::trim(key));
        let Some(idx) = REGISTERED.iter().position(|r| r.0 == lower) else {
            return self.sp.plain.get_variable(key).cloned();
        };
        if PLAIN_REGISTERED.contains(&lower.as_str()) {
            return self.sp.plain.get_variable(key).cloned();
        }
        let (name, ty, text) = REGISTERED[idx];
        if let Some(v) = self.dynamic_value(name, rd) {
            return Some(slot(name, v));
        }
        let value = match ty {
            "Boolean" if text != "?" => SlotValue::Bool(text == "true"),
            "Float" => SlotValue::Float(java::parse_float(text).unwrap_or(0.0)),
            "String" if NULL_STRINGS.contains(&name) => SlotValue::String(None),
            "String" => SlotValue::String(Some(text.to_string())),
            _ => {
                rd.flag(&format!("registered variable {name}"));
                return None;
            }
        };
        Some(slot(name, value))
    }

    /// `gameVars(z)`: the number of the zombie's game variables (the registered ones; the plain ones set later are
    /// counted when present).
    pub fn sp_variable_count(&self) -> usize {
        REGISTERED.len()
            + self
                .sp
                .plain
                .get_game_variables()
                .filter(|s| {
                    !PLAIN_REGISTERED
                        .iter()
                        .any(|r| java::equals_ignore_case(r, &s.key))
                })
                .count()
    }

    /// The `VAR name Type value` lines the probe prints for the zombie's variables, sorted by name (the oracle's
    /// check of the table and the callbacks at construction).
    pub fn sp_variable_dump(&self, rd: &SpRead) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for (name, ty, text) in REGISTERED {
            let value = if text == "?" || ty == "Int" {
                text.to_string()
            } else {
                match self.sp_get_variable(name, rd) {
                    Some(s) => s.get_value_string().unwrap_or_else(|| "null".to_string()),
                    None => "?".to_string(),
                }
            };
            lines.push(format!("VAR {name} {ty} {value}"));
        }
        lines.sort();
        lines
    }

    /// IsoGameCharacter.getVariableBoolean (the action context's variable first): false without a slot.
    pub fn sp_variable_boolean(&self, ctx: Option<&ActionContext>, key: &str, rd: &SpRead) -> bool {
        let s = ctx
            .and_then(|c| c.get_variable(key))
            .or_else(|| self.sp_get_variable(key, rd));
        s.is_some_and(|s| s.get_value_bool())
    }

    /// IsoGameCharacter.getVariableString (the action context's variable first): "" without a slot.
    pub fn sp_variable_string(
        &self,
        ctx: Option<&ActionContext>,
        key: &str,
        rd: &SpRead,
    ) -> Option<String> {
        let s = ctx
            .and_then(|c| c.get_variable(key))
            .or_else(|| self.sp_get_variable(key, rd));
        match s {
            Some(s) => s.get_value_string(),
            None => Some(String::new()),
        }
    }

    /// IsoGameCharacter.setVariable(String, boolean): a registered variable's setter (a registered variable without
    /// one ignores it), else a plain slot.
    pub fn sp_set_bool(&mut self, key: &str, v: bool) -> Result<(), String> {
        let lower = java::case_insensitive_key(java::trim(key));
        let registered =
            REGISTERED.iter().any(|r| r.0 == lower) && !PLAIN_REGISTERED.contains(&lower.as_str());
        if registered {
            match lower.as_str() {
                "bmoving" => self.sp.is_moving = v,
                "bonfloor" => self.on_floor = v,
                "bknockeddown" => self.sp.knocked_down = v,
                "fallonfront" => self.sp.fall_on_front = v,
                "bbecomecrawler" => self.sp.become_crawler = v,
                "usephysichitreaction" => self.sp.use_physic_hit_reaction = v,
                "attackdiddamage" => self.sp.attack_did_damage = v,
                n if SETTABLE_VARIABLES.contains(&n) => {
                    return Err(format!("setVariable({key}, {v}): the setter is not ported"))
                }
                _ => {}
            }
        } else {
            let sp = &mut self.sp;
            sp.plain.set_variable_bool(&mut sp.pool, key, v);
        }
        Ok(())
    }

    /// IsoGameCharacter.setVariable(String, String)
    pub fn sp_set_string(&mut self, key: &str, v: &str) -> Result<(), String> {
        let lower = java::case_insensitive_key(java::trim(key));
        let registered =
            REGISTERED.iter().any(|r| r.0 == lower) && !PLAIN_REGISTERED.contains(&lower.as_str());
        if registered {
            match lower.as_str() {
                "attackoutcome" => self.sp.attack_outcome = v.to_string(),
                "attackdiddamage" => {
                    self.sp.attack_did_damage = string_utils::try_parse_boolean(Some(v))
                }
                "bmoving" => self.sp.is_moving = string_utils::try_parse_boolean(Some(v)),
                "bonfloor" => self.on_floor = string_utils::try_parse_boolean(Some(v)),
                "hitreaction" => self.sp.hit_reaction = v.to_string(),
                "fallonfront" => self.sp.fall_on_front = string_utils::try_parse_boolean(Some(v)),
                "bbecomecrawler" => {
                    self.sp.become_crawler = string_utils::try_parse_boolean(Some(v))
                }
                "usephysichitreaction" => {
                    self.sp.use_physic_hit_reaction = string_utils::try_parse_boolean(Some(v))
                }
                n if SETTABLE_VARIABLES.contains(&n) => {
                    return Err(format!(
                        "setVariable({key}, \"{v}\"): the setter is not ported"
                    ))
                }
                _ => {}
            }
        } else {
            let sp = &mut self.sp;
            sp.plain.set_variable_string(&mut sp.pool, key, v);
        }
        Ok(())
    }

    /// IsoGameCharacter.clearVariable: the slot's `clear()` (a settable callback gets the default of its type through
    /// its setter, a plain slot its zero value, a read-only callback is left alone).
    pub fn sp_clear_variable(&mut self, key: &str) -> Result<(), String> {
        let lower = java::case_insensitive_key(java::trim(key));
        let registered =
            REGISTERED.iter().any(|r| r.0 == lower) && !PLAIN_REGISTERED.contains(&lower.as_str());
        if registered {
            match lower.as_str() {
                "attackdiddamage" => self.sp.attack_did_damage = false,
                "attackoutcome" => self.sp.attack_outcome = String::new(),
                "bmoving" => self.sp.is_moving = false,
                "bonfloor" => self.on_floor = false,
                "fallonfront" => self.sp.fall_on_front = false,
                "bbecomecrawler" => self.sp.become_crawler = false,
                "usephysichitreaction" => self.sp.use_physic_hit_reaction = false,
                "hitreaction" => self.sp.hit_reaction = String::new(),
                n if SETTABLE_VARIABLES.contains(&n) => {
                    return Err(format!("clearVariable({key}): the setter is not ported"))
                }
                _ => {}
            }
        } else {
            self.sp.plain.clear_variable(key);
        }
        Ok(())
    }
}

impl IsoZombie {
    /// IsoGameCharacter.isTwisting: `abs(getTargetTwist()) > 1.0F`
    fn is_twisting(&self) -> bool {
        let twist = self
            .sp
            .anim_player
            .as_ref()
            .map_or(0.0, |p| to_degrees(p.target_twist_angle));
        pz_abs(twist) > 1.0f32
    }

    fn target_twist_degrees(&self) -> f32 {
        self.sp
            .anim_player
            .as_ref()
            .map_or(0.0, |p| to_degrees(p.target_twist_angle))
    }

    /// IsoGameCharacter.isBehaviourMoving: `currentState.isMoving(this)` (LungeState and WalkTowardState true,
    /// PathFindState `isMoving()`).
    fn is_behaviour_moving(&self) -> bool {
        match self.current_state {
            Some(ZombieState::Lunge) | Some(ZombieState::WalkToward) => true,
            Some(ZombieState::PathFind) => self.sp.is_moving,
            _ => false,
        }
    }

    /// IsoGameCharacter.shouldBeTurning (a zombie: not `blockTurning`, no player movement, `aimAtFloor` false;
    /// `ZombieFallDownState` is not ported).
    fn should_be_turning(&self) -> bool {
        let is_twisting = self.is_twisting();
        if self.current_state == Some(ZombieState::FallDown) || self.sp.block_turning {
            false
        } else if self.is_behaviour_moving() {
            is_twisting
        } else if self.current_state == Some(ZombieState::Attack) {
            // isAttacking(): `!this.aimAtFloor`
            true
        } else {
            let abs_excess_twist = self
                .sp
                .anim_player
                .as_ref()
                .map_or(0.0, |p| pz_abs(to_degrees(p.excess_twist)));
            if abs_excess_twist > 1.0f32 {
                true
            } else if self.sp.is_turning {
                is_twisting
            } else {
                false
            }
        }
    }

    /// IsoGameCharacter.shouldBeTurning90
    fn should_be_turning90(&self) -> bool {
        if !self.sp.is_turning {
            false
        } else if self.sp.is_turning90 {
            true
        } else {
            self.target_twist_degrees().abs() > 65.0f32
        }
    }

    /// IsoGameCharacter.shouldBeTurningAround
    fn should_be_turning_around(&self) -> bool {
        if !self.sp.is_turning {
            false
        } else {
            let target_twist_abs = self.target_twist_degrees().abs();
            if self.sp.is_turning_around {
                target_twist_abs > 45.0f32
            } else {
                target_twist_abs > 110.0f32
            }
        }
    }

    /// IsoGameCharacter.setTurningAround: starting a 180 turn raises the global anim event `Turn180Started`; a turn that
    /// is under way raises `Turn180TargetChanged` (and ends) when the direction moved more than 90 degrees from the one
    /// it started towards.
    fn set_turning_around(
        &mut self,
        anim: &mut ZombieAnim,
        rd: &SpRead,
        rand: &mut RandAbstract<Random>,
        is_turning_around: bool,
    ) -> Result<(), String> {
        let is_different = self.sp.is_turning_around != is_turning_around;
        let was_turning_around = self.sp.is_turning_around;
        if is_different || was_turning_around {
            self.sp.is_turning_around = is_turning_around;
            let previous_target_angle = self.sp.initial_turning_around_target;
            let current_target_angle = to_degrees(self.direction_angle_radians());
            if is_different && is_turning_around {
                self.invoke_global_anim_event(anim, rd, rand, "Turn180Started")?;
                self.sp.initial_turning_around_target = current_target_angle;
            }
            if !is_different && was_turning_around {
                let angle_diff = pz_math::rad_to_deg(pz_math::get_closest_angle(
                    pz_math::deg_to_rad(previous_target_angle),
                    pz_math::deg_to_rad(current_target_angle),
                ));
                if pz_abs(angle_diff) > 90.0f32 {
                    self.invoke_global_anim_event(anim, rd, rand, "Turn180TargetChanged")?;
                    self.sp.is_turning_around = false;
                }
            }
        }
        Ok(())
    }

    /// IsoGameCharacter.invokeGlobalAnimEvent -> AdvancedAnimator.invokeGlobalAnimEvent: `invokeAnimEvent(null, null,
    /// event)` calls the one callback the animator has, the character's `OnAnimEvent`, with no layer.
    fn invoke_global_anim_event(
        &mut self,
        anim: &mut ZombieAnim,
        rd: &SpRead,
        rand: &mut RandAbstract<Random>,
        name: &str,
    ) -> Result<(), String> {
        let event = AnimEvent {
            event_name: Some(name.to_string()),
            time: Some(AnimEventTime::End),
            ..AnimEvent::default()
        };
        let mut owner = SpOwner {
            z: self,
            ctx: &mut anim.action_context,
            rd,
        };
        owner.on_anim_event(-1, None, None, &event, rand)
    }

    /// `getAnimationPlayer()`: the animation player is made on first use.
    pub(crate) fn sp_ensure_anim_player(&mut self) {
        if self.sp.anim_player.is_none() {
            self.sp.anim_player = Some(AnimationPlayer::default());
        }
    }

    /// IsoGameCharacter.updateForwardDirection of the zombie's animation player: `AnimationPlayer.updateForwardDirection`
    /// (`setTargetDirection(forward)`, `characterAllowsTwist`, `shoulderTwistWeight`).
    pub(crate) fn sp_update_player_forward(&mut self) {
        let fwd = self.character.forward_direction;
        if let Some(p) = self.sp.anim_player.as_mut() {
            // IsoZombie: `isCharacterAllowsTwist` false, shoulderTwistWeight from `getShoulderTwistWeight()`
            p.update_forward_direction(fwd, false, 0.75);
        }
    }

    /// One `preupdate(); update(); postupdate();` of the single-player zombie in the world's update loop.
    pub fn tick_sp(
        &mut self,
        world: &mut ZombieWorld,
        squares: &dyn GetSquare,
    ) -> Result<(), String> {
        let spw = world
            .sp
            .clone()
            .ok_or("IsoZombie.tick_sp: the world has no single-player part")?;
        let rd = SpRead::new(squares, &spw);
        self.preupdate(world)?;
        self.update_sp(world, &rd)?;
        rd.check()?;
        self.postupdate_sp(world, &rd)?;
        rd.check()
    }

    /// zombie.characters.IsoZombie.update -> updateInternal for the single-player zombie with a plain player target
    /// (no sounds, no corpses, no other zombies, no vehicles, no doors or windows in reach).
    fn update_sp(&mut self, world: &mut ZombieWorld, rd: &SpRead) -> Result<(), String> {
        let mult = world.multiplier;
        let thirty = world.thirty_fps_multiplier();
        if super::iso_zombie::zombie_inactivity_phase(&world.lore)? != self.inactive {
            return Err(
                "IsoZombie.makeInactive: the zombie's activity phase changed (not ported)".into(),
            );
        }
        // doDeferredMovement: the root movement the animation player accumulated goes to the next position
        // (`updateEmitter`, `checkZombieEntersPlayerBuilding`, `updateMovementStatistics`, `OnZombieUpdate` change
        // nothing the oracle shows)
        let updating = self
            .anim
            .as_ref()
            .is_some_and(|a| a.animation_updating_this_frame);
        self.do_deferred_movement(updating, rd);
        self.bonus_spot_time = pz_math::clamp(self.bonus_spot_time - mult, 0.0, f32::MAX);
        self.time_since_seen_flesh =
            pz_math::clamp(self.time_since_seen_flesh + mult, 0.0, f32::MAX);
        // super.update() = IsoGameCharacter.updateInternal: the state timer, then StateMachine.update
        self.character.moving.state_event_delay_timer -= thirty;
        // IsoGameCharacter.updateInternal / IsoMovingObject.update: `lastX/Y/Z` are where the zombie stands now
        self.sp.last_x = self.character.moving.x;
        self.sp.last_y = self.character.moving.y;
        self.sp.last_z = self.character.moving.z;
        self.state_execute(world, rd)?;
        let cur = self.current_state;
        // `cur != StaggerBackState && != BurntToDeath && != FakeDeadZombieState && != ZombieFallDownState && !=
        // ZombieOnGroundState && != ZombieHitReactionState && != ZombieGetUpState` guards everything below (the
        // walk animation choice, the thump test, `damageSheetRope`, the target vector, `separate`, the corpse search)
        if matches!(
            cur,
            Some(ZombieState::StaggerBack)
                | Some(ZombieState::FallDown)
                | Some(ZombieState::OnGround)
                | Some(ZombieState::GetUp)
        ) {
            return Ok(());
        }
        // `cur == PathFindState && finder.progress == notyetfound`: PlayAnim("ZombieIdle"), then
        // `def.animFrameIncrease = 0.08F + Rand.Next(1000) / 8000.0F; *= 0.5F`
        if cur == Some(ZombieState::PathFind)
            && self.path.finder_progress
                == crate::zombie::pathfind::path_find_behavior2::PathFindProgress::NotYetFound
        {
            let inc = 0.08f32 + world.rand.next(1000) as f32 / 8000.0f32;
            let _ = inc * 0.5f32;
        }
        // tryThump(null): in the moving states (PathFind, Lunge) the feeler tile is `getFeelerTile(getFeelersize())`,
        // the tile `(next - last)` ahead: this zombie does not move, so the vector is zero, the tile is the zombie's own
        // square and `testCollideSpecialObjects(self)` is null: no thump, no climb.
        self.damage_sheet_rope(world, rd.squares)?;
        self.allow_repath_delay = pz_math::clamp(self.allow_repath_delay - mult, 0.0, f32::MAX);
        if self.time_since_seen_flesh > self.memory as f32 && self.sp.target {
            self.sp.target = false;
        }
        if self.sp.target {
            self.sp.vector_to_target.x = rd.world.player.x;
            self.sp.vector_to_target.y = rd.world.player.y;
            self.sp.vector_to_target.x -= self.character.moving.x;
            self.sp.vector_to_target.y -= self.character.moving.y;
            if self.speed_type == 1 {
                return Err(
                    "IsoZombie.updateZombieTripping: a sprinter with a target is not ported".into(),
                );
            }
        }
        // (farming plant and campfire of the square, setLastHeardSound, RespondToSound (no world sound exists),
        // ZombieGroupManager.update (a lone zombie): UNKNOWN)
        self.time_since_respond_to_sound += thirty;
        self.separate(rd)?;
        self.update_search_for_corpse_sp(thirty)
    }

    /// IsoZombie.updateSearchForCorpse: with a target the timer is reset; without one it counts down.
    fn update_search_for_corpse_sp(&mut self, thirty: f32) -> Result<(), String> {
        if self.crawling || self.sp.target {
            self.check_for_corpse_timer = 10000.0;
            return Ok(());
        }
        self.check_for_corpse_timer -= thirty;
        if self.check_for_corpse_timer <= 0.0 {
            return Err("IsoZombie.updateSearchForCorpse: the corpse scan is not ported".into());
        }
        Ok(())
    }

    /// StateMachine.update: the current state's `execute`.
    fn state_execute(&mut self, world: &mut ZombieWorld, rd: &SpRead) -> Result<(), String> {
        match self.current_state {
            None => Ok(()),
            Some(ZombieState::Idle) => self.idle_execute_sp(world, rd),
            Some(ZombieState::PathFind) => self.path_find_execute(),
            Some(ZombieState::Lunge) => self.lunge_execute(world, rd),
            Some(ZombieState::Attack) => self.attack_execute(rd),
            Some(ZombieState::FaceTarget) => self.face_target_execute(rd),
            Some(ZombieState::WalkToward) => self.walk_toward_execute(world, rd),
            Some(ZombieState::StaggerBack) => {
                self.stagger_back_execute();
                Ok(())
            }
            // ZombieFallDownState and ZombieGetUpState have no execute (the base State's is empty)
            Some(ZombieState::FallDown) | Some(ZombieState::GetUp) => Ok(()),
            Some(ZombieState::OnGround) => self.on_ground_execute(world, rd),
        }
    }

    fn state_enter(&mut self, state: ZombieState, world: &mut ZombieWorld) -> Result<(), String> {
        match state {
            ZombieState::Idle => {
                self.idle_enter(world);
                Ok(())
            }
            ZombieState::PathFind => self.path_find_enter(),
            ZombieState::Lunge => {
                self.lunge_enter();
                Ok(())
            }
            ZombieState::Attack => self.attack_enter(),
            ZombieState::FaceTarget => Ok(()),
            ZombieState::WalkToward => self.walk_toward_enter(),
            ZombieState::StaggerBack => {
                self.stagger_back_enter();
                Ok(())
            }
            ZombieState::FallDown => {
                self.fall_down_enter(world);
                Ok(())
            }
            ZombieState::OnGround => self.on_ground_enter(world),
            ZombieState::GetUp => self.get_up_enter(),
        }
    }

    fn state_exit(&mut self, state: ZombieState) -> Result<(), String> {
        match state {
            ZombieState::Idle | ZombieState::Lunge | ZombieState::FaceTarget => Ok(()),
            ZombieState::PathFind => self.path_find_exit(),
            ZombieState::Attack => self.attack_exit(),
            ZombieState::WalkToward => self.walk_toward_exit(),
            ZombieState::StaggerBack => {
                self.stagger_back_exit();
                Ok(())
            }
            ZombieState::FallDown => {
                self.fall_down_exit();
                Ok(())
            }
            // ZombieOnGroundState.exit is empty
            ZombieState::OnGround => Ok(()),
            ZombieState::GetUp => {
                self.get_up_exit();
                Ok(())
            }
        }
    }

    /// zombie.ai.StateMachine.changeRootState(newState, restart = false): leave the current state, remember it as the
    /// previous, enter the new one. (The `OnAIStateChange` Lua event: no handler modelled, UNKNOWN.)
    pub(crate) fn change_root_state(
        &mut self,
        new_state: Option<ZombieState>,
        world: &mut ZombieWorld,
    ) -> Result<(), String> {
        if self.current_state == new_state {
            return Ok(());
        }
        let previous = self.current_state;
        if let Some(p) = previous {
            self.state_exit(p)?;
        }
        self.sp.previous_state = previous;
        self.current_state = new_state;
        if let Some(n) = new_state {
            self.state_enter(n, world)?;
        }
        Ok(())
    }

    /// StateMachineComponent.actionStateChanged: `AdvancedAnimator.setState(root, children)`, then the AI state the
    /// root action state is registered for (`initializeStates`) becomes the state machine's current state.
    fn action_state_changed_sp(
        &mut self,
        anim: &mut ZombieAnim,
        world: &mut ZombieWorld,
    ) -> Result<(), String> {
        let root = anim.action_context.get_current_state_name();
        let children = anim.action_context.get_child_state_names();
        anim.animator.set_state_names(&root, &children)?;
        let ai = super::iso_zombie_states::ai_state_for(&root)?;
        if !children.is_empty() {
            return Err(format!(
                "IsoZombie.actionStateChanged: sub states {children:?} are not ported"
            ));
        }
        self.change_root_state(Some(ai), world)
    }

    /// IsoZombie.postupdate -> IsoGameCharacter.postUpdateInternal: on an animation update frame the animation
    /// player, the turning flags, the action context, the animator and the player's angles; then
    /// `canSeeTarget = isTargetVisible()`.
    fn postupdate_sp(&mut self, world: &mut ZombieWorld, rd: &SpRead) -> Result<(), String> {
        let mut anim = self
            .anim
            .take()
            .ok_or("IsoZombie.postupdate: the zombie was made without the animation assets")?;
        // super.postupdate() (IsoMovingObject): the movement is realised before postUpdateAnimating
        if let Err(e) = self.moving_postupdate(world, rd) {
            self.anim = Some(anim);
            return Err(e);
        }
        let r = if anim.animation_updating_this_frame {
            self.post_update_animating_sp(&mut anim, world, rd)
        } else {
            Ok(())
        };
        self.anim = Some(anim);
        r?;
        // IsoZombie.postUpdateInternal: `canSeeTarget = isTargetVisible()`: `getCurrentSquare().isCouldSee(playerIndex)`;
        // the squares' LOS is never computed headless (measured false for every square in the scenarios)
        self.sp.can_see_target = false;
        // `targetSeenTime > 0 && !canSeeTarget`: reset
        if self.sp.target_seen_time > 0.0 && !self.sp.can_see_target {
            self.sp.target_seen_time = 0.0;
        }
        Ok(())
    }

    fn post_update_animating_sp(
        &mut self,
        anim: &mut ZombieAnim,
        world: &mut ZombieWorld,
        rd: &SpRead,
    ) -> Result<(), String> {
        self.sp_ensure_anim_player();
        self.sp_update_player_forward();
        // updateVerticalAimAngle: current and target are both 0
        let should_be_turning = self.should_be_turning();
        self.sp.is_turning = should_be_turning;
        let t90 = self.should_be_turning90();
        self.sp.is_turning90 = t90;
        let ta = self.should_be_turning_around();
        self.set_turning_around(anim, rd, &mut world.rand, ta)?;
        let changed = {
            let view = SpView { z: self, rd };
            anim.action_context.update(&view)
        };
        rd.check()?;
        if changed {
            self.sp.previous_action_state = anim.action_context.peek_previous_state_name();
            self.action_state_changed_sp(anim, world)?;
        }
        let dt = world.time_delta() * anim.animation_time_scale;
        {
            let mut owner = SpOwner {
                z: self,
                ctx: &mut anim.action_context,
                rd,
            };
            let mut env = Env {
                owner: &mut owner,
                rand: &mut world.rand,
            };
            anim.animator.update(dt, &mut env)?;
        }
        rd.check()?;
        for e in [
            "ActiveAnimFinished",
            "ActiveAnimFinishing",
            "ActiveAnimLooped",
        ] {
            anim.action_context.clear_event(e);
        }
        self.apply_deltas();
        // updateAnimPlayer: updateForwardDirection, then AnimationPlayer.Update(deltaT): the tracks, then DoAngles
        self.sp_update_player_forward();
        {
            let mut owner = SpOwner {
                z: self,
                ctx: &mut anim.action_context,
                rd,
            };
            let mut env = Env {
                owner: &mut owner,
                rand: &mut world.rand,
            };
            anim.animator.update_animation_player(dt, &mut env)?;
        }
        rd.check()?;
        // updateAnimation_NonVisualOnly: DoAngles, then calculateDeferredMovement
        if let Some(p) = self.sp.anim_player.as_mut() {
            p.do_angles(dt);
            anim.animator.calculate_deferred_movement(p)?;
        }
        Ok(())
    }

    /// IsoGameCharacter.applyDeltas: the animation player's step deltas from the character's move and turn deltas
    /// (the ported states' `getDeltaModifiers` change nothing; no path to turn to an obstacle).
    fn apply_deltas(&mut self) {
        let move_delta = self.sp.move_delta;
        let mut turn_delta = self.character.turn_delta_normal;
        let twist_delta = turn_delta * 1.8f32;
        if !self.sp.is_turning {
            turn_delta = 0.0;
        }
        let movement_turn_multiplier = f32::max(1.0f32 - move_delta / 2.0f32, 0.0f32);
        let max_twist = (std::f64::consts::PI / 180.0) as f32 * self.sp.max_twist;
        if let Some(p) = self.sp.anim_player.as_mut() {
            p.angle_step_delta = movement_turn_multiplier * turn_delta;
            p.angle_twist_delta = movement_turn_multiplier * twist_delta;
            p.max_twist_angle = max_twist;
        }
    }

    /// IsoGameCharacter.pathToAux (single-player, a zombie): the line between the zombie and the goal decides whether it
    /// walks straight (`bPathfind` false, `setMoving(true)`) or needs the path finder (`bPathfind` true,
    /// `setMoving(false)`).
    pub(crate) fn path_to_aux(
        &mut self,
        x: f32,
        y: f32,
        z: f32,
        rd: &SpRead,
    ) -> Result<(), String> {
        let m = &self.character.moving;
        let (mx, my, mz) = (m.x, m.y, m.z);
        let mut b_line_clear = true;
        // IsoUtils.DistanceManhatten: `abs(x1 - x2) + abs(y1 - y2)`
        let manhatten = (x - mx).abs() + (y - my).abs();
        // the chunk test is the nested `if` of the Java, one condition here
        if pz_math::fastfloor(z) == pz_math::fastfloor(mz)
            && manhatten <= 30.0f32
            && rd
                .world
                .has_chunk_for_square(pz_math::fastfloor(x), pz_math::fastfloor(y))
        {
            // flags: 1 (ignore doors) | 2 (close to walls); a zombie has no cost check
            b_line_clear = !crate::zombie::pathfind::line_clear_collide_main::is_not_clear_old(
                rd.squares,
                &rd.world.alias,
                mx,
                my,
                x,
                y,
                pz_math::fastfloor(z),
                3,
            )?;
        }
        if b_line_clear {
            if let Some(cur) = Self::sp_square(rd.squares, mx, my, mz) {
                if cur.has_stairs() {
                    return Err(
                        "IsoGameCharacter.pathToAux: the zombie stands on stairs (not ported)"
                            .into(),
                    );
                }
            }
            // CanUsePathfindState(): !GameServer.server
            self.sp_set_bool("bPathfind", false)?;
            self.sp_set_bool("bMoving", true)
        } else {
            self.sp_set_bool("bPathfind", true)?;
            self.sp_set_bool("bMoving", false)
        }
    }
}

/// The zombie's variables as the action context's conditions read them: the callbacks and the plain variables.
struct SpView<'a> {
    z: &'a IsoZombie,
    rd: &'a SpRead<'a>,
}

impl IAnimationVariableSource for SpView<'_> {
    fn get_variable(&self, key: &str) -> Option<AnimationVariableSlot> {
        self.z.sp_get_variable(key, self.rd)
    }
}

/// The zombie as the animator's character: its action context's state variables first, then its own variables;
/// anim events run the registered handlers, report to the action context and reach the current AI state.
struct SpOwner<'a> {
    z: &'a mut IsoZombie,
    ctx: &'a mut ActionContext,
    rd: &'a SpRead<'a>,
}

impl SpOwner<'_> {
    /// IsoGameCharacter.OnAnimEvent_PlaySound: `getEmitter().playSoundImpl(file, this)`.
    fn play_sound_impl(
        &mut self,
        file: Option<&str>,
        rand: &mut RandAbstract<Random>,
    ) -> Result<(), String> {
        play_sound_impl(self.rd.world.game_sounds.as_ref(), file, rand)
    }
}

/// CharacterSoundEmitter.playSoundImpl(file, proxy) -> FMODSoundEmitter.playSoundImpl: `GameSounds.getSound(file)` is
/// null for a null or empty name (nothing is played, no draw); else `getRandomClip()` draws `Rand.Next(clips.size())`
/// and `addSound` draws nothing. A sound no script defines is made by `getOrCreateSound` with one clip. The sound's
/// own playing (FMOD) changes no state this port keeps.
pub(crate) fn play_sound_impl(
    game_sounds: Option<&Rc<GameSounds>>,
    file: Option<&str>,
    rand: &mut RandAbstract<Random>,
) -> Result<(), String> {
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return Ok(());
    };
    let sounds = game_sounds.ok_or("PlaySound: the sound scripts are not loaded")?;
    match sounds.sound_by_name.get(file) {
        Some(&index) => {
            sounds.sound(index).get_random_clip(rand)?;
        }
        None => {
            rand.next(1);
        }
    }
    Ok(())
}

impl IAnimationVariableSource for SpOwner<'_> {
    fn get_variable(&self, key: &str) -> Option<AnimationVariableSlot> {
        self.ctx
            .get_variable(key)
            .or_else(|| self.z.sp_get_variable(key, self.rd))
    }
}

impl AnimatorOwner for SpOwner<'_> {
    fn is_player_ready(&self) -> bool {
        self.z.sp.anim_player.is_some()
    }

    /// IsoGameCharacter.OnAnimEvent: the handler registered for the event's name, then `ActionContext.reportEvent`,
    /// then `StateMachine.stateAnimEvent` (the current state's `animEvent` for the base layer).
    fn on_anim_event(
        &mut self,
        layer_depth: i32,
        layer_state_name: Option<&str>,
        _track: Option<&AnimationTrack>,
        event: &AnimEvent,
        rand: &mut RandAbstract<Random>,
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
            match (&event.kind, name.to_ascii_lowercase().as_str()) {
                (
                    AnimEventKind::SetVariable {
                        variable_reference,
                        set_variable_value,
                    },
                    "setvariable",
                ) => {
                    if variable_reference.sub_variable_source_name.is_some() {
                        return Err("SetVariable: a sub variable source is not ported".into());
                    }
                    self.z
                        .sp_set_string(&variable_reference.name, set_variable_value)?;
                }
                (_, "clearvariable") => {
                    if let Some(p) = event.parameter_value.as_deref() {
                        let r = AnimationVariableReference::from_raw_variable_name(p);
                        if r.sub_variable_source_name.is_some() {
                            return Err("ClearVariable: a sub variable source is not ported".into());
                        }
                        self.z.sp_clear_variable(&r.name)?;
                    }
                }
                // IsoZombie.DoFootstepSound(type): the sound manager's listener test and the zombie footstep manager's
                // queue (audio only; no state, no random draw: the oracle's draw counts cover it)
                (_, "footstep") => {}
                // IsoGameCharacter.OnAnimEvent_PlaySound: `getEmitter().playSoundImpl(file, this)`
                (_, "playsound") => self.play_sound_impl(event.parameter_value.as_deref(), rand)?,
                // OnAnimEvent_PlaySoundNoBlend asks the layer whether it blends: no zombie animation raises it
                (_, "playsoundnoblend") => {
                    return Err(
                        "IsoGameCharacter.OnAnimEvent_PlaySoundNoBlend is not ported".into(),
                    )
                }
                // IsoGameCharacter.OnAnimEvent_FallOnFront / _SetOnFloor / _SetKnockedDown: the boolean listeners
                // (`Boolean.parseBoolean(event.parameterValue)`); `GameClient.client` is false
                (_, "fallonfront") => {
                    self.z.sp.fall_on_front = parse_boolean(event.parameter_value.as_deref())
                }
                (_, "setonfloor") => {
                    self.z.on_floor = parse_boolean(event.parameter_value.as_deref())
                }
                (_, "setknockeddown") => {
                    self.z.sp.knocked_down = parse_boolean(event.parameter_value.as_deref())
                }
                // OnAnimEvent_IsAlmostUp: setOnFloor(false), setKnockedDown(false), setSitOnGround(false)
                (_, "isalmostup") => {
                    self.z.on_floor = false;
                    self.z.sp.knocked_down = false;
                }
                _ => {
                    return Err(format!(
                        "IsoGameCharacter.OnAnimEvent: the handler of '{name}' is not ported"
                    ))
                }
            }
        }
        self.ctx.report_event_for_state(layer_state_name, name);
        if layer_depth <= 0 {
            // StateMachine.stateAnimEvent: the current state's animEvent
            if self.z.current_state == Some(ZombieState::Attack) {
                self.z.attack_anim_event(self.ctx, self.rd, event)?;
            }
            // ZombieFallDownState.animEvent: FallOnFront (as the character's listener), PlayDeathSound (a death)
            if self.z.current_state == Some(ZombieState::FallDown) {
                if java::equals_ignore_case(name, "FallOnFront") {
                    self.z.sp.fall_on_front = parse_boolean(event.parameter_value.as_deref());
                }
                if java::equals_ignore_case(name, "PlayDeathSound") {
                    return Err(
                        "ZombieFallDownState.animEvent: PlayDeathSound (a death) is not ported"
                            .into(),
                    );
                }
            }
            // StaggerBackState.animEvent: FallOnFront, SetState (an FMOD parameter)
            if self.z.current_state == Some(ZombieState::StaggerBack)
                && java::equals_ignore_case(name, "FallOnFront")
            {
                self.z.sp.fall_on_front = parse_boolean(event.parameter_value.as_deref());
            }
        }
        Ok(())
    }

    /// AnimationVariableReference.setVariable(character, boolean): the character's variable of that name.
    fn set_variable_bool(
        &mut self,
        reference: &AnimationVariableReference,
        value: bool,
    ) -> Result<(), String> {
        if reference.sub_variable_source_name.is_some() {
            return Err(
                "AnimationVariableReference.setVariable: a sub variable source is not ported"
                    .into(),
            );
        }
        if self.ctx.get_variable(&reference.name).is_some() {
            return Err(
                "AnimationVariableReference.setVariable: an action state's variable slot is not ported"
                    .into(),
            );
        }
        self.z.sp_set_bool(&reference.name, value)
    }

    /// IAnimationVariableSlot.setValue(float) of the existing `maxTwist` slot: `setMaxTwist(degrees)`.
    fn set_variable_float(&mut self, key: &str, value: f32) -> Result<(), String> {
        if java::equals_ignore_case(key, "maxTwist") {
            self.z.sp.max_twist = value;
            Ok(())
        } else {
            Err(format!(
                "IAnimationVariableSlot.setValue(float) of '{key}': not ported"
            ))
        }
    }
}
