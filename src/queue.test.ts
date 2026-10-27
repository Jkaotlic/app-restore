import { describe, expect, it } from "vitest";
import type { Job } from "./app";
import { appsCount, togglePause, etaMinutes, overallText, pendingText, remainingBytes, resumedJob, SpeedMeter } from "./queue";
import type { JobDto, RowDto } from "./types";

const MB = 1024 * 1024;
const job = (id: number, state: JobDto, size: number | null, extra: Partial<RowDto> = {}): Job => ({
  track_id: id,
  name: `A${id}`,
  row: { size_bytes: size, has_saved_copy: false, source: "purchase", ...extra } as RowDto,
  state,
  fromSaved: null,
});

describe("SpeedMeter", () => {
  it("averages bytes per second across downloads", () => {
    const m = new SpeedMeter();
    expect(m.bps).toBeNull();
    m.sample(1, 0, 0);
    expect(m.bps).toBeNull();
    m.sample(1, 10 * MB, 1000);
    expect(m.bps).toBe(10 * MB);
    m.sample(1, 10 * MB, 2000); // стоит на месте — скорость падает, но не до нуля сразу
    expect(m.bps).toBeGreaterThan(0);
    expect(m.bps!).toBeLessThan(10 * MB);
    // следующее скачивание продолжает ту же оценку, а не начинает с нуля
    const before = m.bps;
    m.sample(2, 0, 2500);
    expect(m.bps).toBe(before);
    m.sample(2, 5 * MB, 3000);
    expect(m.bps).not.toBe(before);
  });
  it("skips gaps after sleep or pause and restarts of a job", () => {
    const m = new SpeedMeter();
    m.sample(1, 0, 0);
    m.sample(1, MB, 1000);
    const v = m.bps;
    m.sample(1, 2 * MB, 120_000); // Mac спал: не «1 МБ за 2 минуты»
    expect(m.bps).toBe(v);
    m.sample(1, 0, 121_000); // скачивание началось заново
    expect(m.bps).toBe(v);
    m.sample(1, 0, 121_000); // то же время — без деления на ноль
    expect(m.bps).toBe(v);
  });
});

describe("ETA", () => {
  it("sums what is left to download; unknown size hides it", () => {
    const jobs = [
      job(1, { kind: "downloading", bytes: 30 * MB }, 100 * MB),
      job(2, { kind: "queued" }, 50 * MB),
      job(3, { kind: "installed" }, 70 * MB),
      job(4, { kind: "paused" }, 20 * MB),
      job(5, { kind: "waiting", seconds: 5 }, 10 * MB),
      // каталог с копией ставится без скачивания
      job(6, { kind: "queued" }, 999 * MB, { source: "removed", has_saved_copy: true }),
      job(7, { kind: "installing", percent: 10 }, 999 * MB),
    ];
    expect(remainingBytes(jobs)).toBe((70 + 50 + 20 + 10) * MB);
    expect(remainingBytes([...jobs, job(8, { kind: "queued" }, null)])).toBeNull();
    // больше скачанного, чем ждали (размер из Lookup неточен) — не минус
    expect(remainingBytes([job(1, { kind: "downloading", bytes: 120 * MB }, 100 * MB)])).toBe(0);
  });
  it("minutes round up; unknown or nothing left is null", () => {
    expect(etaMinutes(240 * MB, 1 * MB)).toBe(4);
    expect(etaMinutes(1, 1 * MB)).toBe(1);
    expect(etaMinutes(240 * MB, null)).toBeNull();
    expect(etaMinutes(240 * MB, 0)).toBeNull();
    expect(etaMinutes(null, MB)).toBeNull();
    expect(etaMinutes(0, MB)).toBeNull();
  });
  it("overall line", () => {
    expect(overallText(3, 12, 4, false)).toBe("3 из 12 · осталось ≈ 4 мин");
    expect(overallText(3, 12, null, false)).toBe("3 из 12");
    expect(overallText(3, 12, 4, true)).toBe("3 из 12 · на паузе");
    expect(overallText(12, 12, null, false)).toBe("12 из 12");
  });
});

describe("togglePause", () => {
  it("flips only after the backend agreed", async () => {
    const cmds: string[] = [];
    expect(await togglePause(false, async (c) => void cmds.push(c))).toBe(true);
    expect(await togglePause(true, async (c) => void cmds.push(c))).toBe(false);
    expect(cmds).toEqual(["pause_queue", "resume_queue"]);
  });
  it("keeps the old value when the call fails", async () => {
    await expect(togglePause(false, () => Promise.reject(new Error("x")))).rejects.toThrow("x");
  });
});

describe("unfinished from last time", () => {
  it("counts apps in Russian", () => {
    expect(appsCount(1)).toBe("1 приложение");
    expect(appsCount(3)).toBe("3 приложения");
    expect(appsCount(5)).toBe("5 приложений");
    expect(appsCount(11)).toBe("11 приложений");
    expect(appsCount(21)).toBe("21 приложение");
    expect(appsCount(112)).toBe("112 приложений");
    expect(pendingText(2)).toBe("В прошлый раз не закончили: 2 приложения");
  });
  it("resumed job is queued with a plain icon row", () => {
    const j = resumedJob({ track_id: 7, name: "Банк", download_only: true, size_bytes: null });
    expect(j.track_id).toBe(7);
    expect(j.state).toEqual({ kind: "queued" });
    expect(j.downloadOnly).toBe(true);
    expect(j.row.name).toBe("Банк");
    expect(j.row.letter).toBeNull();
    expect(j.row.size_bytes).toBeNull();
    // размер известен — оценка времени для продолженных тоже есть
    expect(resumedJob({ track_id: 8, name: "Такси", download_only: false, size_bytes: 300 }).row.size_bytes).toBe(300);
  });
});
