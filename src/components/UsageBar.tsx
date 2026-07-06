import { fmtWeighted } from "../util";

export default function UsageBar({
  label,
  pct,
  weighted,
  budget,
  sub,
  source,
}: {
  label: string;
  pct: number;
  weighted: number;
  budget: number;
  sub?: string | null;
  source?: string;
}) {
  const clamped = Math.max(0, Math.min(pct, 100));
  const cls = pct >= 85 ? "bar-red" : pct >= 60 ? "bar-yellow" : "bar-green";
  const live = source === "live";
  return (
    <div className="usage-bar">
      <div className="usage-bar-head">
        <span>
          {label}
          {live && <span className="live-tag" title="Real usage from Claude Code's status line">LIVE</span>}
        </span>
        <span className="dim">
          {/* real % has no weighted-token figure — show just the percentage */}
          {live ? `${pct.toFixed(1)}%` : `${fmtWeighted(weighted)} / ${fmtWeighted(budget)} · ${Math.round(pct)}%`}
          {sub ? ` · ${sub}` : ""}
        </span>
      </div>
      <div className="bar-track">
        <div className={`bar-fill ${cls}`} style={{ width: `${clamped}%` }} />
      </div>
    </div>
  );
}
