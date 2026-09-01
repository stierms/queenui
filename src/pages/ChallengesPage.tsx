import { useEffect, useState } from "react";
import {
  Activity,
  ChevronDown,
  CircleDot,
  Crosshair,
  Swords,
  Timer,
} from "lucide-react";
import { EmptyPage } from "../components/EmptyPage";
import { TimeControlPresets } from "../components/TimeControlPresets";
import type { BusyKeys } from "../hooks/useActionRunner";
import { countLiveGames } from "../lib/chess";
import { durationShortSeconds, timeOfDay } from "../lib/format";
import {
  defaultSelectedTimeControl,
  timeControlValue,
} from "../lib/timeControls";
import {
  campaignEventClass,
  type AppSnapshot,
  type CampaignEvent,
  type CampaignSettings,
  type TimeControl,
} from "../types";
import { Button, Switch } from "../ui/primitives";
import { schedulerHealthDetail, schedulerHealthTitle } from "./schedulerHealth";

type ScheduleMode = "manual" | "time" | "games";
type DurationUnit = "minutes" | "hours";

export function ChallengesPage({
  snapshot,
  timeControls,
  busy,
  onDirectChallenge,
  onStart,
  onStop,
}: {
  snapshot: AppSnapshot;
  timeControls: TimeControl[];
  busy: BusyKeys;
  onDirectChallenge: () => void;
  onStart: (settings: CampaignSettings) => Promise<boolean>;
  onStop: (accountId: string) => Promise<boolean>;
}) {
  const [chosenAccountId, setChosenAccountId] = useState("");
  const accountId = chosenAccountId || (snapshot.accounts[0]?.id ?? "");
  const saved = snapshot.campaigns.find(
    (campaign) => campaign.accountId === accountId,
  );
  const [minRating, setMinRating] = useState(1800);
  const [maxRating, setMaxRating] = useState(2600);
  const [concurrency, setConcurrency] = useState(1);
  const defaultClock = timeControlValue(
    defaultSelectedTimeControl(timeControls),
  );
  const [clock, setClock] = useState(defaultClock);
  /*
   * Rated, matching the backend's own default for a campaign: both
   * `CampaignSettings::default` and the serde default that fills the field in
   * for campaigns persisted before it existed (`default_campaign_rated`) say
   * true. The form used to open on Casual while the backend opened on Rated, so
   * the two disagreed about what "the default" was — and a casual campaign
   * moves no rating, which is not what an operator arming matchmaking is
   * usually after.
   *
   * A saved campaign always wins: the adjust-during-render block below assigns
   * `saved.rated` verbatim, so this value is only ever seen by an account that
   * has never had a campaign stored.
   */
  const [rated, setRated] = useState(true);
  const [color, setColor] = useState("random");
  const [acceptIncoming, setAcceptIncoming] = useState(false);
  const [scheduleMode, setScheduleMode] = useState<ScheduleMode>("manual");
  const [runDuration, setRunDuration] = useState(1);
  const [durationUnit, setDurationUnit] = useState<DurationUnit>("hours");
  const [runGames, setRunGames] = useState(10);
  const [now, setNow] = useState(() => Date.now());

  // Adjust form state during render when the persisted campaign for the
  // selected account changes (see react.dev "adjusting state when props change").
  const savedSignature = saved
    ? [
        accountId,
        saved.minRating,
        saved.maxRating,
        saved.concurrency,
        saved.clockLimit,
        saved.clockIncrement,
        saved.rated,
        saved.color,
        saved.acceptIncomingChallenges,
        saved.stopAfterMinutes ?? "manual",
        saved.stopAfterGames ?? "manual",
      ].join(":")
    : `${accountId}:none:${defaultClock}`;
  const [prevSignature, setPrevSignature] = useState<string | null>(null);
  if (prevSignature !== savedSignature) {
    setPrevSignature(savedSignature);
    if (saved) {
      setMinRating(saved.minRating);
      setMaxRating(saved.maxRating);
      setConcurrency(saved.concurrency);
      setClock(`${saved.clockLimit / 60}+${saved.clockIncrement}`);
      setRated(saved.rated);
      setColor(saved.color);
      setAcceptIncoming(saved.acceptIncomingChallenges);
      if (saved.stopAfterMinutes !== null) {
        setScheduleMode("time");
        if (saved.stopAfterMinutes >= 60 && saved.stopAfterMinutes % 60 === 0) {
          setRunDuration(saved.stopAfterMinutes / 60);
          setDurationUnit("hours");
        } else {
          setRunDuration(saved.stopAfterMinutes);
          setDurationUnit("minutes");
        }
      } else if (saved.stopAfterGames !== null) {
        setScheduleMode("games");
        setRunGames(saved.stopAfterGames);
      } else {
        setScheduleMode("manual");
      }
    } else {
      setMinRating(1800);
      setMaxRating(2600);
      setConcurrency(1);
      setClock(defaultClock);
      // The same default as the initial state above, for the same reason:
      // this branch runs when the selected account has no saved campaign.
      setRated(true);
      setColor("random");
      setAcceptIncoming(false);
      setScheduleMode("manual");
      setRunDuration(1);
      setDurationUnit("hours");
      setRunGames(10);
    }
  }

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  if (!snapshot.accounts.length) {
    return (
      <EmptyPage
        icon={<Crosshair />}
        title="Connect a bot first"
        copy="Automatic matchmaking needs a configured Lichess BOT account and UCI engine."
      />
    );
  }

  const runtime = snapshot.campaignRuntimes.find(
    (item) => item.accountId === accountId,
  );
  const running = Boolean(runtime && runtime.status !== "stopped");
  /*
   * This account's live games, counted from the snapshot like every other
   * surface in the app. The panel used to print the scheduler's own
   * `activeGames`, which `stop_campaign` leaves untouched (it clears
   * `pendingChallenges` only) — so after "Stop matchmaking" this page kept
   * reporting "Active games 2" for as long as the app stayed open, while the
   * sidebar badge, the Overview status strip and the Games page all counted
   * down to 0 as those games finished. The scheduler's counter is still what
   * the health line reports, where it describes a controller that is running.
   */
  const liveGameCount = countLiveGames(snapshot.games, accountId);
  const selectedAccount = snapshot.accounts.find(
    (account) => account.id === accountId,
  );
  const [limitMinutes, increment] = clock.split("+").map(Number);
  const stopAfterMinutes =
    scheduleMode === "time"
      ? runDuration * (durationUnit === "hours" ? 60 : 1)
      : null;
  const stopAfterGames = scheduleMode === "games" ? runGames : null;
  const scheduleValid =
    (scheduleMode !== "time" ||
      (stopAfterMinutes !== null &&
        Number.isInteger(stopAfterMinutes) &&
        stopAfterMinutes >= 1 &&
        stopAfterMinutes <= 10_080)) &&
    (scheduleMode !== "games" ||
      (stopAfterGames !== null &&
        Number.isInteger(stopAfterGames) &&
        stopAfterGames >= 1 &&
        stopAfterGames <= 10_000));
  const configurationValid =
    minRating >= 0 &&
    maxRating <= 5000 &&
    minRating <= maxRating &&
    scheduleValid;
  const nextScanSeconds = runtime?.nextScanAt
    ? Math.max(0, Math.ceil((runtime.nextScanAt - now) / 1000))
    : null;
  const stopSeconds = runtime?.stopAt
    ? Math.max(0, Math.ceil((runtime.stopAt - now) / 1000))
    : null;
  const scheduleSummary =
    scheduleMode === "time"
      ? `${runDuration} ${durationUnit === "hours" ? (runDuration === 1 ? "hour" : "hours") : runDuration === 1 ? "minute" : "minutes"}`
      : scheduleMode === "games"
        ? `${runGames} game${runGames === 1 ? "" : "s"}`
        : "manual stop";
  const runLimitDisplay =
    running && runtime?.stopAt && stopSeconds !== null
      ? `${timeOfDay(runtime.stopAt)} · ${durationShortSeconds(stopSeconds)} left`
      : scheduleMode === "games"
        ? `${runtime?.gamesStarted ?? 0} / ${runGames} games`
        : scheduleMode === "time"
          ? scheduleSummary
          : "Manual stop";

  function startMatchmaking() {
    if (!configurationValid || running) return;
    void onStart({
      accountId,
      minRating,
      maxRating,
      concurrency,
      clockLimit: limitMinutes * 60,
      clockIncrement: increment,
      rated,
      color,
      acceptIncomingChallenges: acceptIncoming,
      stopAfterMinutes,
      stopAfterGames,
    });
  }

  return (
    <div className="module-content challenge-center">
      <section className="module-hero challenge-hero">
        <div>
          <span className="eyebrow">Continuous matchmaking</span>
          <h2>Automatic challenge mode</h2>
          <p>
            Keep the selected number of games or pending challenges filled with
            random online bots in your chosen strength range.
          </p>
        </div>
        <Button
          variant="secondary"
          className="h-[34px]"
          onClick={onDirectChallenge}
        >
          <Swords size={15} />
          Direct challenge
        </Button>
      </section>

      <section className="campaign-layout">
        <form
          className="panel campaign-form"
          onSubmit={(event) => {
            event.preventDefault();
            startMatchmaking();
          }}
        >
          <div className="panel-heading">
            <div>
              <span className="eyebrow">Campaign setup</span>
              <h2>Matchmaking rules</h2>
            </div>
            <Crosshair size={20} />
          </div>
          <div className="campaign-fields">
            <label className="campaign-account">
              <span>Play with bot</span>
              <div className="select-wrap">
                <select
                  value={accountId}
                  disabled={running}
                  onChange={(event) => setChosenAccountId(event.target.value)}
                >
                  {snapshot.accounts.map((account) => (
                    <option value={account.id} key={account.id}>
                      {account.username}
                    </option>
                  ))}
                </select>
                <ChevronDown size={15} />
              </div>
            </label>

            <fieldset disabled={running}>
              <legend>Opponent strength</legend>
              <div className="rating-range">
                <label>
                  <span>Minimum rating</span>
                  <input
                    type="number"
                    min="0"
                    max="5000"
                    step="50"
                    value={minRating}
                    onChange={(event) =>
                      setMinRating(Number(event.target.value))
                    }
                  />
                </label>
                <i>to</i>
                <label>
                  <span>Maximum rating</span>
                  <input
                    type="number"
                    min="0"
                    max="5000"
                    step="50"
                    value={maxRating}
                    onChange={(event) =>
                      setMaxRating(Number(event.target.value))
                    }
                  />
                </label>
              </div>
              <small className="field-hint">
                Uses the selected clock’s established, non-provisional Lichess
                rating.
              </small>
            </fieldset>

            <fieldset disabled={running}>
              <legend>Parallel capacity</legend>
              <div className="concurrency-picker">
                {[1, 2, 3, 4, 5, 6, 7, 8].map((value) => (
                  <button
                    type="button"
                    className={concurrency === value ? "selected" : ""}
                    aria-pressed={concurrency === value}
                    onClick={() => setConcurrency(value)}
                    key={value}
                  >
                    {value}
                  </button>
                ))}
              </div>
              <small className="field-hint">
                Counts both active games and unanswered challenges. Finished
                slots refill automatically.
              </small>
            </fieldset>

            <fieldset disabled={running}>
              <legend>Time control</legend>
              <TimeControlPresets
                controls={timeControls}
                selected={clock}
                onSelect={setClock}
                className="campaign-clocks"
              />
              {/*
               * A saved campaign clock that is not a whole number of minutes
               * (say 90+0 seconds) matches no preset, so the fieldset showed
               * nothing selected while a campaign ran on exactly that clock.
               * Name the running clock instead of showing an empty choice.
               */}
              {!timeControls.some(
                (control) => timeControlValue(control) === clock,
              ) && (
                <small className="field-hint">
                  Running on {clock}, which is not one of the presets above.
                  Picking a preset replaces it.
                </small>
              )}
            </fieldset>

            <div className="campaign-options">
              <div className="button-field">
                <span>Color</span>
                <div className="segmented">
                  {["white", "random", "black"].map((item) => (
                    <button
                      type="button"
                      disabled={running}
                      className={color === item ? "selected" : ""}
                      aria-pressed={color === item}
                      key={item}
                      onClick={() => setColor(item)}
                    >
                      {item}
                    </button>
                  ))}
                </div>
              </div>
              <div className="button-field">
                <span>Mode</span>
                <div className="segmented">
                  {[false, true].map((item) => (
                    <button
                      type="button"
                      disabled={running}
                      className={rated === item ? "selected" : ""}
                      aria-pressed={rated === item}
                      key={String(item)}
                      onClick={() => setRated(item)}
                    >
                      {item ? "Rated" : "Casual"}
                    </button>
                  ))}
                </div>
              </div>
            </div>

            <div className="campaign-toggle-row">
              <div>
                <strong>Accept matching incoming challenges</strong>
                <small>
                  Incoming Bot API challenges must match this campaign’s rating,
                  clock, mode, and color rules. They use the same capacity and
                  run limit as outgoing challenges.
                </small>
              </div>
              <Switch
                checked={acceptIncoming}
                disabled={running}
                aria-label="Accept matching incoming challenges"
                onCheckedChange={setAcceptIncoming}
              />
            </div>

            <fieldset className="campaign-schedule" disabled={running}>
              <legend>Run limit</legend>
              <div className="segmented campaign-schedule-modes">
                {(
                  [
                    ["manual", "Until stopped"],
                    ["time", "Time limit"],
                    ["games", "Game limit"],
                  ] as const
                ).map(([mode, label]) => (
                  <button
                    type="button"
                    className={scheduleMode === mode ? "selected" : ""}
                    aria-pressed={scheduleMode === mode}
                    key={mode}
                    onClick={() => setScheduleMode(mode)}
                  >
                    {label}
                  </button>
                ))}
              </div>
              {scheduleMode === "time" && (
                <div className="campaign-limit-editor">
                  <label>
                    <span>Run for</span>
                    <input
                      type="number"
                      min="1"
                      max={durationUnit === "hours" ? 168 : 10_080}
                      step="1"
                      value={runDuration}
                      onChange={(event) =>
                        setRunDuration(Number(event.target.value))
                      }
                    />
                  </label>
                  <div className="segmented campaign-duration-units">
                    {(["minutes", "hours"] as const).map((unit) => (
                      <button
                        type="button"
                        className={durationUnit === unit ? "selected" : ""}
                        aria-pressed={durationUnit === unit}
                        key={unit}
                        onClick={() => {
                          if (unit === durationUnit) return;
                          setRunDuration((value) =>
                            unit === "hours"
                              ? Math.max(1, Math.ceil(value / 60))
                              : value * 60,
                          );
                          setDurationUnit(unit);
                        }}
                      >
                        {unit}
                      </button>
                    ))}
                  </div>
                </div>
              )}
              {scheduleMode === "games" && (
                <div className="campaign-limit-editor campaign-game-limit">
                  <label>
                    <span>Stop after games started</span>
                    <input
                      type="number"
                      min="1"
                      max="10000"
                      step="1"
                      value={runGames}
                      onChange={(event) =>
                        setRunGames(Number(event.target.value))
                      }
                    />
                  </label>
                </div>
              )}
              <small className="field-hint">
                When the limit is reached, QueenUI cancels unanswered outgoing
                challenges and lets already-started games finish normally.
              </small>
            </fieldset>

            <div className="campaign-safety">
              <CircleDot size={16} />
              <p>
                <strong>Opponent protection is always active</strong>
                <small>
                  Only online bots are considered. QueenUI spaces requests,
                  avoids repeat opponents for 15 minutes, and backs off on
                  Lichess rate limits.
                </small>
              </p>
            </div>
          </div>
          <div className="campaign-submit">
            <p className="campaign-confirmation">
              {running
                ? "These settings are locked while matchmaking runs."
                : configurationValid
                  ? `Ready: ${minRating}–${maxRating} · ${clock} · ${rated ? "rated" : "casual"} · ${concurrency} slot${concurrency === 1 ? "" : "s"} · ${acceptIncoming ? "incoming + outgoing" : "outgoing only"} · ${scheduleSummary} — start from the Live controller.`
                  : "Fix the rating range or run limit to enable matchmaking."}
            </p>
          </div>
        </form>

        <aside className="panel campaign-monitor">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">Live controller</span>
              <h2>{selectedAccount?.username}</h2>
            </div>
            <div className="campaign-controls">
              <span
                className={`campaign-state state-${runtime?.status ?? "stopped"}`}
              >
                <i />
                {runtime?.status ?? "stopped"}
              </span>
              {running ? (
                <Button
                  variant="danger"
                  disabled={busy.has(`campaign-${accountId}`)}
                  onClick={() => void onStop(accountId)}
                >
                  {runtime?.status === "stopping"
                    ? "Stopping…"
                    : "Stop matchmaking"}
                </Button>
              ) : (
                <Button
                  variant="primary"
                  disabled={
                    !configurationValid || busy.has(`campaign-${accountId}`)
                  }
                  onClick={startMatchmaking}
                >
                  <Crosshair size={16} />
                  {busy.has(`campaign-${accountId}`)
                    ? "Starting…"
                    : "Start matchmaking"}
                </Button>
              )}
            </div>
          </div>
          <div className="capacity-visual">
            <div className="capacity-ring">
              <strong>
                {liveGameCount + (runtime?.pendingChallenges ?? 0)}
              </strong>
              <span>of {concurrency}</span>
            </div>
            <div>
              <strong>Occupied slots</strong>
              <small>
                The scheduler fills empty capacity until the configured run
                limit is reached.
              </small>
            </div>
          </div>
          <div
            className={`scheduler-health health-${runtime?.status ?? "stopped"}`}
          >
            <span className="health-indicator">
              <i />
            </span>
            <p>
              <strong>{schedulerHealthTitle(runtime)}</strong>
              <small>
                {schedulerHealthDetail(runtime, nextScanSeconds, now)}
              </small>
            </p>
          </div>
          <div className="campaign-stats">
            <div>
              <span>Online scanned</span>
              <strong>{runtime?.onlineBotsScanned ?? 0}</strong>
            </div>
            <div>
              <span>Eligible now</span>
              <strong>{runtime?.eligibleBots ?? 0}</strong>
            </div>
            <div>
              <span>Active games</span>
              <strong>{liveGameCount}</strong>
            </div>
            <div>
              <span>Pending</span>
              <strong>{runtime?.pendingChallenges ?? 0}</strong>
            </div>
            <div>
              <span>Sent this run</span>
              <strong>{runtime?.challengesSent ?? 0}</strong>
            </div>
            <div>
              <span>Games this run</span>
              <strong>{runtime?.gamesStarted ?? 0}</strong>
            </div>
            <div>
              <span>Run limit</span>
              <strong className="campaign-run-limit">
                {runtime?.stopAt && running && <Timer size={13} />}
                {runLimitDisplay}
              </strong>
            </div>
            <div>
              <span>Next scan</span>
              <strong>
                {nextScanSeconds === null
                  ? "—"
                  : durationShortSeconds(nextScanSeconds)}
              </strong>
            </div>
          </div>
          <div className="campaign-activity">
            <Activity size={16} />
            <p>
              <span>Current activity</span>
              <strong>
                {runtime?.activity ?? "Configure the campaign and press Start."}
              </strong>
              {runtime?.lastOpponent && (
                <small>Latest opponent: {runtime.lastOpponent}</small>
              )}
            </p>
          </div>
          {/* Pushed by a snapshot event, not by anything the operator just
              did, so it is announced and says what the text belongs to —
              the scheduler's own words were previously dropped in bare. */}
          {runtime?.error && (
            <p className="campaign-error" role="alert">
              <strong>Matchmaking reported a problem</strong> {runtime.error}
            </p>
          )}
          <CampaignActivityFeed events={runtime?.events ?? []} />
        </aside>
      </section>
    </div>
  );
}

export function CampaignActivityFeed({ events }: { events: CampaignEvent[] }) {
  return (
    <section className="matchmaking-feed">
      <div className="feed-heading">
        <h3>Matchmaking activity</h3>
        {events.length > 0 && (
          <span className="feed-live">
            <i />
            Live
          </span>
        )}
      </div>
      {events.length === 0 ? (
        <div className="feed-empty">
          <Activity size={17} />
          <p>
            No activity yet — discovery scans and opponent responses appear
            here.
          </p>
        </div>
      ) : (
        <div className="feed-events">
          {[...events].reverse().map((event) => (
            <article
              className={`feed-event ${campaignEventClass(event.kind)}`}
              key={event.id}
            >
              <span className="event-marker">
                <i />
              </span>
              <div>
                <header>
                  <strong>{event.title}</strong>
                  <time>
                    {new Date(event.timestamp).toLocaleTimeString([], {
                      hour: "2-digit",
                      minute: "2-digit",
                      second: "2-digit",
                    })}
                  </time>
                </header>
                {event.detail && <p>{event.detail}</p>}
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
