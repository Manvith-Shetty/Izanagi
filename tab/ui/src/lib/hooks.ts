import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { followFeed, loadCensus, loadFeed, loadOverview } from "./api";
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

export function useOverview() {
  return useQuery({ queryKey: ["overview"], queryFn: loadOverview, refetchInterval: 4000 });
}

export function useCensus() {
  return useQuery({ queryKey: ["census"], queryFn: loadCensus, refetchInterval: 60_000 });
}

/** The merged feed: history once, then live over SSE. Newest first. */
export function useFeed() {
  const [items, setItems] = useState<FeedItem[]>([]);
  const [sample, setSample] = useState(false);
  const [live, setLive] = useState(false);
  const seen = useRef(new Set<number>());

  useEffect(() => {
    let stop = () => {};
    let cancelled = false;
    loadFeed().then((r) => {
      if (cancelled) return;
      r.data.forEach((i) => seen.current.add(i.id));
      setItems([...r.data].reverse());
      setSample(r.sample);
      if (r.sample) return;
      stop = followFeed(
        (i) => {
          if (seen.current.has(i.id)) return;
          seen.current.add(i.id);
          setItems((prev) => [i, ...prev].slice(0, 300));
        },
        setLive,
      );
    });
    return () => {
      cancelled = true;
      stop();
    };
  }, []);

  return { items, sample, live };
}
