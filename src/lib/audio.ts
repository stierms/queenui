let moveAudioContext: AudioContext | undefined;

/** Web Audio is unavailable in jsdom and stripped-down WebViews. */
export function audioAvailable() {
  return typeof AudioContext !== "undefined";
}

function getMoveAudioContext() {
  moveAudioContext ??= new AudioContext();
  return moveAudioContext;
}

/** Resume the shared context (no-op when Web Audio is unavailable). */
export function resumeMoveAudio() {
  if (!audioAvailable()) return;
  void getMoveAudioContext().resume();
}

export function playMoveSound(capture: boolean) {
  if (!audioAvailable()) return;
  const context = getMoveAudioContext();
  const play = () => {
    const now = context.currentTime;
    const oscillator = context.createOscillator();
    const gain = context.createGain();
    oscillator.type = capture ? "triangle" : "sine";
    oscillator.frequency.setValueAtTime(capture ? 185 : 520, now);
    oscillator.frequency.exponentialRampToValueAtTime(
      capture ? 72 : 230,
      now + (capture ? 0.11 : 0.055),
    );
    gain.gain.setValueAtTime(capture ? 0.18 : 0.11, now);
    gain.gain.exponentialRampToValueAtTime(
      0.0001,
      now + (capture ? 0.12 : 0.065),
    );
    oscillator.connect(gain).connect(context.destination);
    oscillator.start(now);
    oscillator.stop(now + (capture ? 0.125 : 0.07));
  };
  if (context.state === "suspended") void context.resume().then(play);
  else play();
}
