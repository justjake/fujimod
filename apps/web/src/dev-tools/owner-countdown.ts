export interface Countdown {
  message: string;
  remaining: number;
}

/** One owner action at a time. Cancellation rejects the controller's request. */
export function createOwnerCountdown(
  display: (state: Countdown | undefined) => void,
  speak: (text: string) => void = (text) => {
    if (typeof speechSynthesis === 'undefined') throw Error('Browser speech unavailable');
    speechSynthesis.speak(new SpeechSynthesisUtterance(text));
  },
  silence: () => void = () => globalThis.speechSynthesis?.cancel(),
) {
  let cancel: (() => void) | undefined;
  return {
    start(message: string): Promise<void> {
      if (cancel) return Promise.reject(Error('Owner countdown already running'));
      return new Promise((resolve, reject) => {
        let timer: ReturnType<typeof setTimeout>;
        const clear = () => {
          clearTimeout(timer);
          cancel = undefined;
          display(undefined);
        };
        cancel = () => {
          clear();
          silence();
          reject(Error('Owner countdown interrupted'));
        };
        let remaining = 10;
        const step = () => {
          if (remaining === 0) {
            clear();
            resolve();
            return;
          }
          try {
            display({ message, remaining });
            speak(String(remaining--));
            timer = setTimeout(step, 1000);
          } catch (error) {
            clear();
            silence();
            reject(error);
          }
        };
        try {
          silence();
          step();
        } catch (error) {
          clear();
          reject(error);
        }
      });
    },
    cancel: () => cancel?.(),
  };
}
