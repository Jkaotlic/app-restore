// Запуск: события Rust → общее состояние → текущий экран.
import { listen } from "@tauri-apps/api/event";
import { call, currentScreen, go, refreshSidebar, sessionChecked, startScreen, state } from "./app";
import { h } from "./dom";
import { jobInbox } from "./jobs";
import "./screens/accounts";
import "./screens/allcopies";
import "./screens/choose";
import "./screens/done";
import "./screens/login";
import "./screens/phone";
import { warningSeen } from "./screens/warning";
import "./styles.css";
import type { Account, JobEvent, OwnedEvent, PhonesEvent, PurchasesProgress, RowsDto } from "./types";

async function wire(): Promise<void> {
  await listen<JobEvent>("job", ({ payload }) => {
    // неизвестный номер: задание ещё регистрируется (или прошлого захода) — ждёт в jobInbox
    if (jobInbox.apply(state.jobs, payload)) currentScreen()?.onJob?.();
  });
  await listen<OwnedEvent>("owned", ({ payload }) => currentScreen()?.onOwned?.(payload));
  await listen<PurchasesProgress>("purchases_progress", ({ payload }) => currentScreen()?.onPurchasesProgress?.(payload));
  await listen<RowsDto>("purchases", ({ payload }) => currentScreen()?.onPurchases?.(payload));
  await listen<PhonesEvent>("phones", ({ payload }) => {
    state.phones = payload.phones;
    state.phonesError = payload.error;
    currentScreen()?.onPhones?.();
  });
}

async function boot(): Promise<void> {
  await wire();
  const first = !warningSeen();
  if (first) go("warning");
  else document.getElementById("app")!.replaceChildren(h("p", { class: "loading body", text: "Открываем…" }));
  try {
    state.account = await call<Account | null>("session_info");
  } catch {
    state.account = null;
  }
  sessionChecked();
  if (first) refreshSidebar();
  else go(await startScreen());
  call<string | null>("check_update")
    .then((tag) => {
      state.update = tag;
      if (tag) refreshSidebar();
    })
    .catch(() => undefined);
}

void boot();
