/** The one "this is unavailable, not a real value" visual treatment in the app:
 * neutral grey, never rendered as ok/pass. Two wire behaviours share it —
 * a component-level `status: "unavailable"` marker inside an otherwise-200
 * composite response (e.g. `Concentration.issuer_overrides`, `Liquidity.nav_status`,
 * a `Check`/`Scenario` row), and a caught 403 `ApiError` from an endpoint denied
 * outright (`useFetch`'s `forbidden`). Both render through this component so a
 * denial never has a second, inconsistent look. */
export const UNAVAILABLE_LABEL = "N/A";
export const UNAVAILABLE_BAR_COLOR = "#9e9e9e";

export default function Unavailable({ reason }: { reason?: string }) {
  return (
    <p className="unavailable">
      {UNAVAILABLE_LABEL}
      {reason ? ` — ${reason}` : ""}
    </p>
  );
}
