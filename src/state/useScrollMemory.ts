import { useEffect, useLayoutEffect, useRef } from 'react';

const SCROLLER = '.app__content';

/**
 * Keeps the page scroll position when the folder on screen changes.
 *
 * `key` identifies what is shown (the folder path) and is `null` while nothing
 * is shown, for example during a rescan. Scroll events are ignored while
 * nothing is shown, so the browser clamping the position as the content
 * shrinks does not overwrite where the user was.
 */
export function useScrollMemory(key: string | null) {
  const remembered = useRef(0);
  const visible = useRef(false);

  useEffect(() => {
    const scroller = document.querySelector<HTMLElement>(SCROLLER);
    if (!scroller) return;
    const onScroll = () => {
      if (visible.current) remembered.current = scroller.scrollTop;
    };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    return () => scroller.removeEventListener('scroll', onScroll);
  }, []);

  useLayoutEffect(() => {
    visible.current = key !== null;
    if (key === null) return;
    const scroller = document.querySelector<HTMLElement>(SCROLLER);
    if (scroller && scroller.scrollTop !== remembered.current) {
      scroller.scrollTop = remembered.current;
    }
  }, [key]);
}
