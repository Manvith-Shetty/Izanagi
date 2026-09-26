import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { followFeed, loadCensus, loadMe } from "./api";
import { sampleFeed } from "./sample";
import type { FeedItem } from "./types";
import { now } from "./format";

/** Wall-clock seconds, ticking once a second: drives every countdown on the page. */
export function useNow(): number {
  const [t, setT] = useState(now);
  useEffect(() => {
    const i = setInterval(() => setT(now()), 1000);
    return () => clearInterval(i);
  }, []);
  return t;
}

/** The signed-in person's wallet and tabs, or why there isn't one. */
export function useMe() {
  return useQuery({ queryKey: ["me"], queryFn: loadMe, refetchInterval: (q) => (q.state.data?.kind === "signed_in" ? 4000 : false) });
}

export function useCensus() {
  return useQuery({ queryKey: ["census"], queryFn: loadCensus, refetchInterval: 60_000 });
}

/**
 * This person's feed, newest first. Live over SSE when signed in (Tab replays the history
 * first); sample when Tab isn't running; empty when signed out.
 */
export function useFeed(mode: "live" | "sample" | "off") {
  const [items, setItems] = useState<FeedItem[]>([]);
  const [live, setLive] = useState(false);
  const seen = useRef(new Set<number>());

  useEffect(() => {
    seen.current = new Set();
    setLive(false);
    if (mode === "sample") {
      setItems([...sampleFeed()].reverse());
      return;
    }
    setItems([]);
    if (mode === "off") return;
    return followFeed(
      (i) => {
        if (seen.current.has(i.id)) return;
        seen.current.add(i.id);
        setItems((prev) => [i, ...prev].slice(0, 300));
      },
      setLive,
    );
  }, [mode]);

  return { items, live, sample: mode === "sample" };
}
