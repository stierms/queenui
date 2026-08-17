/**
 * Recognising the refusals the operator can answer, and what they must answer
 * *with*.
 *
 * Leaving a remote runner — for this computer, or for a *different* remote
 * runner — is refused while that runner still has work in flight, and refused
 * again if the backend could not reach it to find out. Those refusals are
 * questions: the games do not have to end and the challenges do not have to be
 * cancelled, they only have to be acknowledged as staying where they are, and
 * repeating the save with `acknowledgedRunner` set to that runner's URL is the
 * answer. Every other refusal (`live_games_switch_error`, the local
 * outgoing-challenge and Lichess checks, the mid-swap and interrupted-switch
 * errors) names something that must actually be resolved first, and no
 * acknowledgement clears it.
 *
 * So the panel has to tell them apart, and the only signal it gets is the
 * sentence. These four patterns mirror `verify_remote_handover` in
 * `src-tauri/src/lib.rs` exactly — including the singular/plural nouns and the
 * `it`/`they` pronoun, which the Rust picks by count — and they are anchored, so
 * a refusal that merely resembles one of them is not offered a confirmation that
 * would not clear it. Three of the four are pinned by Rust tests as well, which
 * is what makes those checkable rather than a guess; the games-*and*-challenges
 * sentence is currently produced by `verify_remote_handover` without a Rust test
 * asserting it, so the pattern below is the only thing holding its wording. If it
 * drifts, this classification silently stops offering the confirmation and the
 * refusal lands in the error slot with no way to answer it — worth pinning on the
 * Rust side too.
 *
 * The sentence is also the only place the runner's address appears. The
 * acknowledgement is bound to that address now: the backend compares it against
 * the live remote's own canonical base URL, so a URL remembered from an earlier
 * refusal acknowledges nothing and re-refuses naming whatever is actually there.
 * Extracting the URL from the refusal being answered is what makes the
 * acknowledgement unforgeable in the direction that matters — it can only ever
 * confirm the runner the backend just asked about.
 *
 * The acknowledgement-flow tests in `SettingsPage.test.tsx` assert both
 * directions: these four sentences raise the dialog, and every other refusal the
 * backend can produce reaches the error slot without one.
 */

/**
 * Which of the four questions the sentence is asking. The dialog's title is the
 * panel's own words, so it needs to know which one is on screen; the description
 * is the backend's sentence verbatim and needs nothing.
 */
export type RemoteHandoverRefusalKind =
  "playing" | "challenges" | "playing-and-challenges" | "unverified";

export type RemoteHandoverRefusal = {
  kind: RemoteHandoverRefusalKind;
  /**
   * The runner URL the refusal named, verbatim. This is the canonical spelling
   * the backend dials and compares against, not the operator's — which is the
   * point: an acknowledgement spelled any other way is not one.
   */
  runner: string;
};

/**
 * The four sentences, in the Rust's own order of decision, each capturing the
 * URL it names.
 *
 * "The remote runner at {url} is still playing {count} {game|games}. Confirm
 * that {it|they} will keep playing there before switching runners."
 */
const STILL_PLAYING =
  /^The remote runner at (\S+) is still playing \d+ games?\. Confirm that (?:it|they) will keep playing there before switching runners\.$/;

/**
 * "The remote runner at {url} still owns {count} outgoing {challenge|challenges}.
 * Confirm that {it|they} will remain there before switching runners."
 */
const OWNS_CHALLENGES =
  /^The remote runner at (\S+) still owns \d+ outgoing challenges?\. Confirm that (?:it|they) will remain there before switching runners\.$/;

/**
 * "The remote runner at {url} is still playing {count} {game|games} and owns
 * {count} outgoing {challenge|challenges}. Confirm that this work will remain
 * there before switching runners."
 *
 * Not a composition of the two above: the Rust writes one sentence about both,
 * with a single closing clause ("this work") because no pronoun covers games and
 * challenges together.
 */
const PLAYING_AND_OWNS_CHALLENGES =
  /^The remote runner at (\S+) is still playing \d+ games? and owns \d+ outgoing challenges?\. Confirm that this work will remain there before switching runners\.$/;

/**
 * "Could not verify the remote runner at {url}; it may still be playing games.
 * Confirm that its games will keep running there before switching runners."
 */
const UNVERIFIED =
  /^Could not verify the remote runner at (\S+); it may still be playing games\. Confirm that its games will keep running there before switching runners\.$/;

const PATTERNS: [RegExp, RemoteHandoverRefusalKind][] = [
  [STILL_PLAYING, "playing"],
  [OWNS_CHALLENGES, "challenges"],
  [PLAYING_AND_OWNS_CHALLENGES, "playing-and-challenges"],
  [UNVERIFIED, "unverified"],
];

/**
 * Which acknowledgement this refusal is asking for and for which runner, or
 * `null` when it is not asking for one. Whitespace is trimmed first and nothing
 * else is normalized: the sentence is the backend's, and the panel renders it
 * verbatim.
 */
export function remoteHandoverRefusal(
  message: string,
): RemoteHandoverRefusal | null {
  const sentence = message.trim();
  for (const [pattern, kind] of PATTERNS) {
    const match = pattern.exec(sentence);
    if (match) return { kind, runner: match[1] };
  }
  return null;
}
