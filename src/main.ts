// Запуск: события Rust → общее состояние → текущий экран.
import { listen } from "@tauri-apps/api/event";
import { call, currentScreen, go, refreshSidebar, sessionChecked, startScreen, state } from "./app";
import { dropMessages, mergePending } from "./drop";
import { h } from "./dom";
import { jobInbox } from "./jobs";
import { speedMeter } from "./queue";
import "./screens/accounts";
import "./screens/allcopies";
import "./screens/choose";
import "./screens/done";
import "./screens/login";
import "./screens/phone";
import { warningSeen } from "./screens/warning";
import "./styles.css";
import type { Account, DropDto, JobEvent, OwnedEvent, PhonesEvent, PurchasesProgress, RowsDto, SweepDto } from "./types";

/** Перетаскивание, когда экран сам его не принимает: копия — на «Все копии» (не с первого
 * экрана-предупреждения), номера ждут «Выбора», ошибки — в боковой панели. */
function dropElsewhere(d: DropDto): void {
  if (d.ids.length) state.pendingLinks = mergePending(state.pendingLinks, d.ids);
  state.dropNote = dropMessages(d).join(". ") || null;
  const last = d.copies[d.copies.length - 1];
  if (last && state.screen !== "warning") {
    state.pickCopy = last.path;
    state.beforeCopies = state.screen;
    go("copies");
  } else refreshSidebar();
}

async function wire(): Promise<void> {
  await listen<JobEvent>("job", ({ payload }) => {
    // скорость для «осталось ≈ N мин» — по всем скачиваниям, и до регистрации задания
    if (payload.state.kind === "downloading") speedMeter.sample(payload.track_id, payload.state.bytes, Date.now());
    else speedMeter.stop(payload.track_id);
    // неизвестный номер: задание ещё регистрируется (или прошлого захода) — ждёт в jobInbox
    if (jobInbox.apply(state.jobs, payload)) currentScreen()?.onJob?.();
  });
  await listen<boolean>("queue_paused", ({ payload }) => {
    state.paused = payload;
    currentScreen()?.onJob?.();
  });
  await listen<OwnedEvent>("owned", ({ payload }) => currentScreen()?.onOwned?.(payload));
  await listen<SweepDto>("catalog_check", ({ payload }) => currentScreen()?.onCatalogCheck?.(payload));
  await listen<number[]>("new_releases", ({ payload }) => {
    state.newIds = new Set(payload);
    currentScreen()?.onNewReleases?.();
  });
  await listen<DropDto>("dropped", ({ payload }) => {
    const screen = currentScreen();
    if (screen?.onDrop) screen.onDrop(payload);
    else dropElsewhere(payload);
  });
  await listen<PurchasesProgress>("purchases_progress", ({ payload }) => currentScreen()?.onPurchasesProgress?.(payload));
  await listen<RowsDto>("purchases", ({ payload }) => currentScreen()?.onPurchases?.(payload));
  // окно Связки ключей macOS ждёт ответа (вход, снимок после входа, смена Apple ID); null — всё
  await listen<string | null>("keychain_note", ({ payload }) => {
    state.keychainNote = payload;
    refreshSidebar();
  });
  // вход ipatool определился после шага со Связкой ключей, который доделывался в фоне
  await listen<Account | null>("account", ({ payload }) => {
    state.account = payload;
    refreshSidebar();
    currentScreen()?.onAccount?.();
  });
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
