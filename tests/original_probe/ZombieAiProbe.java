import java.io.PrintStream;
import java.util.Random;
import zombie.characters.IsoPlayer;
import zombie.characters.IsoZombie;
import zombie.characters.SurvivorDesc;
import zombie.core.random.RandAbstract;
import zombie.core.random.RandStandard;
import zombie.core.skinnedmodel.population.OutfitRNG;
import zombie.iso.IsoCamera;
import zombie.iso.IsoCell;
import zombie.iso.IsoChunk;
import zombie.iso.IsoChunkMap;
import zombie.iso.IsoGridSquare;
import zombie.iso.IsoObject;
import zombie.network.GameServer;
import zombie.network.ServerLOS;
import zombie.network.ServerMap;
import zombie.SandboxOptions;

// The ORIGINAL zombie AI run headless: the zombie of a seed on the square (8280, 11670, 0) of the harness's world and a
// player stand-in a fixed offset away, ticked as the single-player game's world ticks its objects (preupdate, update,
// postupdate). `GameServer.server` is false from before the zombie is built until the scenario ends (the zombie's variable
// callbacks, its animation update cadence and the square lookups are the single-player ones then), the world has an
// IsoChunkMap around the player and ServerLOS.init() ran; the dedicated server's world is otherwise as the other probes have
// it. Scenario "zai:<seed>:<ticks>:<dx>:<dy>:<mode>": the player stands at (8280.5 + dx, 11670.5 + dy); `force` sets the
// zombie's target to the player (and timeSinceSeenFlesh to 0) before every tick, `none` leaves the zombie without a target.
// Per tick: the draws of the global Rand, the action context's state, the AI state, position, direction, health, target,
// the path behaviour and the animation variables the states read.
public class ZombieAiProbe {
    static int curTick;

    /** A java.util.Random that prints the Java stack of every draw (PZ_RANDTRACE). */
    static final class TraceRandom extends Random {
        TraceRandom(long seed) { super(seed); }
        private void tr(String k) {
            StringBuilder sb = new StringBuilder("RANDTRACE tick=" + curTick + " " + k);
            for (StackTraceElement e : new Throwable().getStackTrace()) {
                String c = e.getClassName();
                if (c.startsWith("zombie.")) sb.append("\n    ").append(c).append('.').append(e.getMethodName()).append(':').append(e.getLineNumber());
            }
            System.err.println(sb);
        }
        @Override public int nextInt(int bound) { tr("nextInt(" + bound + ")"); return super.nextInt(bound); }
        @Override public float nextFloat() { tr("nextFloat"); return super.nextFloat(); }
    }
    static final String[] VARS = {
        "bLunge", "bMoving", "bPathfind", "bHasTarget", "bCanSeeTarget", "bAttack", "isFacingTarget", "alerted", "bThump",
        "bKnockedDown", "bOnFloor", "bStaggerBack", "hashitreaction", "AttackType", "AttackOutcome", "AttackDidDamage", "ZombieBiteDone",
        "angle", "animAngle", "twist", "targetTwist", "shoulderTwist", "excessTwist", "angleStepDelta", "angleTwistDelta", "isTurning",
        "isTurning90", "isTurningAround", "previousState", "bStopLunging", "lungeTimer", "distanceToTarget", "stateEventDelayTimer"
    };

    static final java.util.Set<String> NO_READ = new java.util.HashSet<>(java.util.Arrays.asList("useragdollvehiclecollision", "isonback", "isupright", "canragdoll", "anygrappleanim", "numtwistbones", "grappleanim"));

    // the variables whose getters make the animation player: read once it exists (the first animation frame makes it), "?" before
    static final String[] RAG = {
        "canRagdoll", "isUpright", "isOnBack", "anyGrappleAnim", "grappleAnim", "numTwistBones", "useRagdollVehicleCollision"
    };

    static String vars(IsoZombie z) {
        StringBuilder sb = new StringBuilder();
        for (String n : VARS) {
            var s = z.getVariable(n);
            sb.append(n).append('=').append(s == null ? "-" : s.getValueString()).append(' ');
        }
        boolean made = z.hasAnimationPlayer();
        for (String n : RAG) {
            sb.append(n).append('=').append(made ? z.getVariable(n).getValueString() : "?").append(' ');
        }
        return sb.toString().trim();
    }

    static void scenario(PrintStream out, String spec, IsoCell cell) throws Exception {
        String[] p = spec.split(":");
        long seed = Long.parseLong(p[1]);
        int ticks = Integer.parseInt(p[2]);
        float dx = Float.parseFloat(p[3]);
        float dy = Float.parseFloat(p[4]);
        boolean force = p[5].equals("force") || p[5].startsWith("hit");
        // `hit<N>[,<M>...][@<weapon>[@<damageSplit>[@<flags>]]]`: like force, and at tick N (and M, ...) the player stand-in hits
        // the zombie through `IsoZombie.Hit(weapon, player, damageSplit, false, 1.0f, false)` with the item of that script
        // (default Base.BaseballBat, 1.0); flags: `c` = the wielder's critical hit is set before the hit
        String[] hm = p[5].split("@");
        java.util.Set<Integer> hitTicks = new java.util.HashSet<>();
        if (hm[0].startsWith("hit")) for (String t : hm[0].substring(3).split(",")) hitTicks.add(Integer.parseInt(t));
        String hitWeapon = hm.length > 1 ? hm[1] : "Base.BaseballBat";
        float hitSplit = hm.length > 2 ? Float.parseFloat(hm[2]) : 1.0f;
        String hitFlags = hm.length > 3 ? hm[3] : "";
        if (!ActionContextProbe.chunksLoaded) {
            ActionContextProbe.loadChunks(cell);
            ActionContextProbe.chunksLoaded = true;
            ServerLOS.init();
        }
        java.util.Map<String, Integer> saved = new java.util.LinkedHashMap<>();
        for (java.util.Map.Entry<String, Integer> e : ActionContextProbe.BASE.entrySet()) {
            Object opt = ActionContextProbe.g(SandboxOptions.instance.lore, Character.toLowerCase(e.getKey().charAt(0)) + e.getKey().substring(1));
            saved.put(e.getKey(), (Integer) opt.getClass().getMethod("getValue").invoke(opt));
            ActionContextProbe.setOption(opt, e.getValue());
        }
        out.println("SCENARIO " + spec);
        ActionContextProbe.options(out);
        // single-player: no dedicated server, one local player with a chunk map around it
        GameServer.server = false;
        IsoPlayer.numPlayers = 1;
        IsoChunkMap cm = new IsoChunkMap(cell);
        cm.playerId = 0;
        cm.ignore = false;
        cm.worldX = 1035;
        cm.worldY = 1458;
        IsoChunk[] arr = (IsoChunk[]) ActionContextProbe.g(cm, "chunksSwapA");
        int w = IsoChunkMap.chunkGridWidth;
        ServerMap sm = ServerMap.instance;
        for (int wx = 1034; wx <= 1035; wx++) {
            for (int wy = 1458; wy <= 1459; wy++) {
                IsoChunk ch = null;
                for (Object o : sm.loadedCells) {
                    ServerMap.ServerCell sc = (ServerMap.ServerCell) o;
                    int cx = wx - sc.wx * 8, cy = wy - sc.wy * 8;
                    if (cx >= 0 && cx < 8 && cy >= 0 && cy < 8 && sc.chunks[cx][cy] != null) ch = sc.chunks[cx][cy];
                }
                arr[w * (wy - (cm.worldY - w / 2)) + (wx - (cm.worldX - w / 2))] = ch;
            }
        }
        cell.chunkMap[0] = cm;
        // the player stand-in first, so its construction draws are not in the zombie's sequence
        int px = (int) Math.floor(8280.5f + dx), py = (int) Math.floor(11670.5f + dy);
        zombie.iso.IsoMovingObject.setIDCount(0);
        IsoPlayer pl = new IsoPlayer(cell, new SurvivorDesc(), px, py, 0);
        IsoGridSquare psq = cell.getGridSquare(px, py, 0);
        pl.setCurrent(psq);
        pl.setMovingSquareNow();
        pl.setX(8280.5f + dx);
        pl.setY(11670.5f + dy);
        pl.setZ(0);
        IsoPlayer.players[0] = pl;
        cell.getObjectList().add(pl);
        IsoCamera.setCameraCharacter(pl);
        java.lang.reflect.Field rf = RandAbstract.class.getDeclaredField("rand");
        rf.setAccessible(true);
        // PZ_RANDTRACE=1: every draw of the global Rand prints its Java stack on stderr (debug aid, not part of the oracle)
        rf.set(RandStandard.INSTANCE, System.getenv("PZ_RANDTRACE") != null ? new TraceRandom(seed) : new Random(seed));
        java.lang.reflect.Field idf = RandAbstract.class.getDeclaredField("id");
        idf.setAccessible(true);
        idf.set(RandStandard.INSTANCE, 0);
        OutfitRNG.setSeed(seed);
        IsoZombie z = new IsoZombie(cell);
        out.println("ZOMBIE draws=" + ActionContextProbe.g(RandStandard.INSTANCE, "id") + " vars=" + ActionContextProbe.gameVars(z) + " id=" + z.getID() + " zombieId=" + ActionContextProbe.g(z, "zombieId"));
        // the zombie's variables at construction: name, slot type and value (the port's table of the registered callbacks)
        java.util.List<String> varLines = new java.util.ArrayList<>();
        for (var slot : z.getGameVariables()) {
            // the getters of NO_READ make the animation player (getAnimationPlayer()), which the first animation frame does in
            // the game: they are not read here
            String v = NO_READ.contains(slot.getKey().toLowerCase()) ? "?" : slot.getValueString();
            if (z.hasAnimationPlayer()) { System.err.println("PLAYER MADE BY " + slot.getKey()); }
            varLines.add("VAR " + slot.getKey().toLowerCase() + " " + slot.getType() + " " + v);
        }
        java.util.Collections.sort(varLines);
        for (String l : varLines) out.println(l);
        IsoGridSquare sq = cell.getGridSquare(8280, 11670, 0);
        z.setCurrent(sq);
        z.setMovingSquareNow();
        z.setX(8280.5f);
        z.setY(11670.5f);
        z.setZ(0);
        cell.getObjectList().add(z);
        Object pfb = z.getPathFindBehavior2();
        java.util.Set<zombie.iso.objects.IsoDeadBody> seenCorpses = java.util.Collections.newSetFromMap(new java.util.IdentityHashMap<>());
        int prev = (Integer) ActionContextProbe.g(RandStandard.INSTANCE, "id");
        for (int n = 1; n <= ticks; n++) {
            curTick = n;
            if (force) {
                z.setTarget(pl);
                ZombieProbe.set(z, IsoZombie.class, "timeSinceSeenFlesh", 0.0f);
            }
            if (hitTicks.contains(n)) {
                try {
                    zombie.inventory.types.HandWeapon bat = (zombie.inventory.types.HandWeapon) zombie.inventory.InventoryItemFactory.CreateItem(hitWeapon);
                    pl.setCriticalHit(hitFlags.contains("c"));
                    if (System.getenv("PZ_HITINFO") != null) {
                        System.err.println("HITINFO shovingMod=" + pl.getShovingMod() + " strongTrait=" + pl.getCharacterTraits().get(zombie.scripting.objects.CharacterTrait.STRONG)
                            + " weakTrait=" + pl.getCharacterTraits().get(zombie.scripting.objects.CharacterTrait.WEAK) + " endurance=" + pl.getStats().get(zombie.characters.CharacterStat.ENDURANCE)
                            + " knockbackAttackMod=" + ActionContextProbe.g(pl, "knockbackAttackMod") + " doShove=" + pl.isDoShove() + " aimAtFloor=" + pl.isAimAtFloor() + " crit=" + pl.isCriticalHit()
                            + " weaponLevel=" + pl.getWeaponLevel() + " bothHands=" + pl.isItemInBothHands(bat) + " twoHand=" + bat.isTwoHandWeapon() + " ranged=" + bat.isRanged()
                            + " staggerMod=" + bat.getStaggerBackTimeMod(pl, z) + " splat=" + bat.isSplatBloodOnNoDeath() + " knockback=" + bat.isKnockBackOnNoDeath() + " crit=" + bat.getCriticalDamageMultiplier()
                            + " hasSharp=" + bat.hasSharpness() + " sharp=" + bat.getSharpness() + " sharpMult=" + bat.getSharpnessMultiplier() + " cond=" + bat.getCondition() + "/" + bat.getConditionMax() + " zhitReaction=" + pl.getVariableString("ZombieHitReaction") + " zdir=" + z.getForwardDirection().x + "," + z.getForwardDirection().y + " hitTime=" + z.getHitTime()
                            + " fwd=" + pl.getForwardDirection().x + "," + pl.getForwardDirection().y + " invulnerable=" + z.isInvulnerable());
                    }
                    float dmg = z.Hit(bat, pl, hitSplit, false, 1.0f, false);
                    out.println("HIT damage=" + dmg + " hp=" + z.getHealth() + " dead=" + z.isDead() + " knockedDown=" + z.isKnockedDown() + " staggerBack=" + z.isStaggerBack() + " hitReaction=" + z.getHitReaction());
                } catch (Throwable t) {
                    out.println("HIT-ERR " + t);
                    t.printStackTrace(System.err);
                }
            }
            z.preupdate();
            z.update();
            z.postupdate();
            int draws = (Integer) ActionContextProbe.g(RandStandard.INSTANCE, "id");
            out.println("T " + n + " draws=" + (draws - prev) + " ctx=" + z.getActionContext().getCurrentStateName()
                + " child=" + ActionContextProbe.list(z.getActionContext().getChildStates())
                + " ai=" + (z.getCurrentState() == null ? "null" : z.getCurrentState().getClass().getSimpleName())
                + " x=" + z.getX() + " y=" + z.getY() + " dir=" + z.getDir()
                + " fwd=" + z.getForwardDirection().x + "," + z.getForwardDirection().y
                + " look=" + z.getLookAngleRadians() + " hp=" + z.getHealth()
                + " target=" + (z.getTarget() == null ? "-" : "player") + " php=" + pl.getHealth()
                + " goal=" + ActionContextProbe.g(pfb, "goal")
                + " " + vars(z));
            prev = draws;
            // a corpse the zombie left on the squares around it (`die()` -> `becomeCorpse()`), printed once when it appears
            for (zombie.iso.objects.IsoDeadBody body : corpses(cell)) {
                if (seenCorpses.add(body)) out.println(corpseLine(n, body));
            }
            // PZ_DBG=<from>:<to>: the movement internals of those ticks on stderr (debug aid, not part of the oracle)
            String dbg = System.getenv("PZ_DBG");
            if (dbg != null) {
                String[] r = dbg.split(":");
                if (n >= Integer.parseInt(r[0]) && n <= Integer.parseInt(r[1])) {
                    System.err.println("DBG " + n + " x=" + z.getX() + " y=" + z.getY() + " nx=" + z.getNextX() + " ny=" + z.getNextY()
                        + " lx=" + z.getLastX() + " ly=" + z.getLastY() + " tx=" + ActionContextProbe.g(pfb, "targetX") + " ty=" + ActionContextProbe.g(pfb, "targetY")
                        + " updating=" + z.isAnimationUpdatingThisFrame() + " path2=" + (z.getPath2() != null)
                        + " impulse=" + z.getImpulsex() + "," + z.getImpulsey());
                }
            }
        }
        out.println("END " + spec + " draws=" + ActionContextProbe.g(RandStandard.INSTANCE, "id"));
        cell.getObjectList().remove(z);
        // the corpse (an `IsoDeadBody` on a square's static moving objects, counted by `CorpseCount`) goes with the scenario
        for (zombie.iso.objects.IsoDeadBody body : corpses(cell)) {
            body.removeFromWorld();
            body.getSquare().getStaticMovingObjects().remove(body);
        }
        // leave no trace for the next scenario of this JVM: the zombie is in the moving-object list of every square it
        // stood on or moved from, and a rustled bush keeps its `objectRenderEffects` (the headless world never updates it)
        sq.getMovingObjects().remove(z);
        for (IsoGridSquare s2 : new IsoGridSquare[] {z.getCurrentSquare(), z.getLastSquare(), z.getMovingSquare()}) {
            if (s2 != null) s2.getMovingObjects().remove(z);
        }
        for (int cx = 8272; cx < 8288; cx++) {
            for (int cy = 11664; cy < 11680; cy++) {
                for (int cz = 0; cz < 8; cz++) {
                    IsoGridSquare s3 = cell.getGridSquare(cx, cy, cz);
                    if (s3 == null) continue;
                    s3.getMovingObjects().remove(z);
                    for (int oi = 0; oi < s3.getObjects().size(); oi++) {
                        IsoObject o3 = s3.getObjects().get(oi);
                        if (o3.getObjectRenderEffects() != null) ZombieProbe.set(o3, IsoObject.class, "objectRenderEffects", null);
                    }
                }
            }
        }
        cell.getObjectList().remove(pl);
        psq.getMovingObjects().remove(pl);
        IsoPlayer.players[0] = null;
        cell.chunkMap[0] = null;
        GameServer.server = true;
        for (java.util.Map.Entry<String, Integer> e : saved.entrySet()) {
            ActionContextProbe.setOption(ActionContextProbe.g(SandboxOptions.instance.lore, Character.toLowerCase(e.getKey().charAt(0)) + e.getKey().substring(1)), e.getValue());
        }
    }

    // the `IsoDeadBody` objects on the squares around the zombie's (8280, 11670, 0)
    static java.util.List<zombie.iso.objects.IsoDeadBody> corpses(IsoCell cell) {
        java.util.List<zombie.iso.objects.IsoDeadBody> list = new java.util.ArrayList<>();
        for (int cx = 8279; cx <= 8281; cx++) {
            for (int cy = 11669; cy <= 11671; cy++) {
                IsoGridSquare s = cell.getGridSquare(cx, cy, 0);
                if (s == null) continue;
                for (int i = 0; i < s.getStaticMovingObjects().size(); i++) {
                    if (s.getStaticMovingObjects().get(i) instanceof zombie.iso.objects.IsoDeadBody b) list.add(b);
                }
            }
        }
        return list;
    }

    // `CORPSE T <tick> ...`: what `IsoDeadBody(died)` copied from the zombie
    static String corpseLine(int tick, zombie.iso.objects.IsoDeadBody b) throws Exception {
        zombie.iso.Vector2 fd = (zombie.iso.Vector2) ActionContextProbe.g(b, "forwardDirection");
        return "CORPSE T " + tick + " x=" + b.getX() + " y=" + b.getY() + " z=" + b.getZ()
            + " sq=" + b.getSquare().getX() + "," + b.getSquare().getY() + "," + b.getSquare().getZ()
            + " angle=" + ActionContextProbe.g(b, "angle") + " fwd=" + fd.x + "," + fd.y
            + " fallOnFront=" + b.isFallOnFront() + " killedByFall=" + b.isKilledByFall() + " crawling=" + b.isCrawling()
            + " ragdollFall=" + ActionContextProbe.g(b, "ragdollFall") + " female=" + b.isFemale()
            + " wasZombie=" + ActionContextProbe.g(b, "wasZombie") + " fakeDead=" + b.isFakeDead() + " skeleton=" + b.isSkeleton()
            + " rotStage=" + ActionContextProbe.g(b, "zombieRotStageAtDeath")
            + " items=" + b.getContainer().getItems().size() + " worn=" + b.getWornItems().size() + " attached=" + b.getAttachedItems().size()
            + " collidable=" + ActionContextProbe.g(b, "collidable") + " solid=" + ActionContextProbe.g(b, "solid");
    }

    // `zsq`: the collide, path and vision matrices the harness's chunk loading left on every square of the four loaded chunks
    // (`loadInWorldStreamerThread`'s own pass sees only the squares of its chunk, so the bits across a chunk border stay as
    // the square was made: blocked for movement), which `IsoGridSquare.testCollideAdjacent` reads
    static void squareMatrices(PrintStream out, IsoCell cell) throws Exception {
        if (!ActionContextProbe.chunksLoaded) {
            ActionContextProbe.loadChunks(cell);
            ActionContextProbe.chunksLoaded = true;
            ServerLOS.init();
        }
        out.println("SCENARIO zsq");
        for (int wx = 1034; wx <= 1035; wx++) {
            for (int wy = 1458; wy <= 1459; wy++) {
                IsoChunk ch = null;
                for (Object o : ServerMap.instance.loadedCells) {
                    ServerMap.ServerCell sc = (ServerMap.ServerCell) o;
                    int cx = wx - sc.wx * 8, cy = wy - sc.wy * 8;
                    if (cx >= 0 && cx < 8 && cy >= 0 && cy < 8 && sc.chunks[cx][cy] != null) ch = sc.chunks[cx][cy];
                }
                out.println("CHUNK " + wx + " " + wy + " levels=" + ch.getMinLevel() + ".." + ch.getMaxLevel());
                for (int z = ch.getMinLevel(); z <= ch.getMaxLevel(); z++) {
                    for (int y = 0; y < 8; y++) {
                        for (int x = 0; x < 8; x++) {
                            IsoGridSquare sq = ch.getGridSquare(x, y, z);
                            if (sq == null) continue;
                            out.println("SQ " + sq.getX() + "," + sq.getY() + "," + sq.getZ()
                                + " c=" + Integer.toHexString((Integer) ActionContextProbe.g(sq, "collideMatrix"))
                                + " p=" + Integer.toHexString((Integer) ActionContextProbe.g(sq, "pathMatrix"))
                                + " v=" + Integer.toHexString((Integer) ActionContextProbe.g(sq, "visionMatrix")));
                        }
                    }
                }
            }
        }
        out.println("END zsq");
    }

    public static void main(String[] a) throws Exception {
        java.util.TimeZone.setDefault(java.util.TimeZone.getTimeZone("UTC"));
        ServerHarness.run(a[0], out -> {
            for (int i = 1; i < a.length; i++) {
                if (a[i].equals("zsq")) squareMatrices(out, zombie.iso.IsoWorld.instance.currentCell);
                else if (a[i].startsWith("zai:")) scenario(out, a[i], zombie.iso.IsoWorld.instance.currentCell);
                else throw new IllegalArgumentException("unknown scenario " + a[i]);
            }
        });
    }
}
