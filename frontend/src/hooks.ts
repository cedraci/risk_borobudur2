import { useCallback, useEffect, useState } from "react";
import { ApiError } from "./api";

export function useFetch<T>(fn: () => Promise<T>, deps: unknown[]) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A 403 is a permission denial, not a bug — distinct from `error` so callers can
  // render it through the same <Unavailable/> treatment as a component-level
  // `status: "unavailable"` marker instead of the red error text.
  const [forbidden, setForbidden] = useState<string | undefined>(undefined);
  const [tick, setTick] = useState(0);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  useEffect(() => {
    let alive = true;
    setError(null);
    setForbidden(undefined);
    fn().then(
      (d) => alive && setData(d),
      (e) => {
        if (!alive) return;
        if (e instanceof ApiError && e.status === 403) {
          setForbidden(e.detail ?? e.message);
        } else {
          setError(String(e?.detail ?? e?.message ?? e));
        }
      },
    );
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { data, error, forbidden, reload };
}
