import { describe, expect, it } from "vitest";
import type { Job } from "./app";
import { JobInbox } from "./jobs";
import type { RowDto } from "./types";

const job = (id: number): Job => ({ track_id: id, name: "A", row: {} as RowDto, state: { kind: "queued" }, fromSaved: null });
const failed = { kind: "failed", error: { code: "device_not_trusted", text: "x", retryable: true }, saved: null } as unknown as Job["state"];

describe("JobInbox", () => {
  it("applies events to known jobs at once", () => {
    const jobs = new Map([[1, job(1)]]);
    const inbox = new JobInbox();
    expect(inbox.apply(jobs, { track_id: 1, state: failed, from_saved: "1.0" })).toBe(true);
    expect(jobs.get(1)?.state).toBe(failed);
    expect(jobs.get(1)?.fromSaved).toBe("1.0");
  });
  it("keeps the last event of an unknown id until the job is registered", () => {
    const jobs = new Map<number, Job>();
    const inbox = new JobInbox();
    expect(inbox.apply(jobs, { track_id: -7, state: { kind: "installing", percent: 3 } as Job["state"], from_saved: null })).toBe(false);
    expect(inbox.apply(jobs, { track_id: -7, state: failed, from_saved: null })).toBe(false);
    const j = job(-7);
    inbox.register(jobs, j);
    expect(jobs.get(-7)?.state).toBe(failed);
    // буфер отдан один раз
    const again = job(-7);
    inbox.register(jobs, again);
    expect(again.state).toEqual({ kind: "queued" });
  });
  it("«повтор через N с» remembers when the wait ends, other states forget it", () => {
    const jobs = new Map([[1, job(1)]]);
    const inbox = new JobInbox();
    inbox.apply(jobs, { track_id: 1, state: { kind: "waiting", seconds: 10 }, from_saved: null }, 5000);
    expect(jobs.get(1)?.waitUntil).toBe(15_000);
    inbox.apply(jobs, { track_id: 1, state: { kind: "downloading", bytes: 1 }, from_saved: null }, 6000);
    expect(jobs.get(1)?.waitUntil).toBeUndefined();
    // пришло до регистрации — отсчёт от прихода события, а не от регистрации
    inbox.apply(jobs, { track_id: 2, state: { kind: "waiting", seconds: 5 }, from_saved: null }, 1000);
    const j2 = job(2);
    inbox.register(jobs, j2);
    expect(j2.waitUntil).toBe(6000);
  });
  it("buffer is capped", () => {
    const jobs = new Map<number, Job>();
    const inbox = new JobInbox(2);
    for (const id of [1, 2, 3]) inbox.apply(jobs, { track_id: id, state: failed, from_saved: null });
    const j1 = job(1);
    inbox.register(jobs, j1);
    expect(j1.state).toEqual({ kind: "queued" });
    const j3 = job(3);
    inbox.register(jobs, j3);
    expect(j3.state).toBe(failed);
  });
});
