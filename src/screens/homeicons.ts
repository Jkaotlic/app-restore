import { asUiError, call, safe, targetPhone } from "../app";
import { h } from "../dom";
import { errorText } from "../labels";
import type { InstalledDto } from "../types";

export interface HomeIcon {
  bundle_id: string;
  name: string;
}

export function homeIconPicker(find: (icon: HomeIcon, itemId: number | null) => Promise<void>) {
  const title = h("h3", { text: "Значки с экрана Домой", attrs: { id: "home-icons-title" } });
  const status = h("p", { class: "note", attrs: { role: "status" } });
  const list = h("div", { class: "home-icons-list" });
  const filter = h("input", { class: "input", attrs: { type: "search", placeholder: "Название или идентификатор", "aria-label": "Фильтр значков" } });
  const all = h("input", { attrs: { type: "checkbox" } });
  const dialog = h("dialog", { class: "home-icons", attrs: { "aria-labelledby": "home-icons-title" } },
    h("div", { class: "workflow-line" }, title, h("button", { class: "link", text: "Закрыть", on: { click: () => dialog.close() } })),
    status, filter, h("label", { class: "icon-filter" }, all, " Включить найденные в списке установленных"), list);
  let icons: HomeIcon[] = [];
  let installed = new Map<string, InstalledDto>();
  let read = 0;
  function render() {
    const q = filter.value.trim().toLowerCase();
    const visible = icons.filter((i) => (all.checked || !installed.has(i.bundle_id)) && (i.name + " " + i.bundle_id).toLowerCase().includes(q));
    list.replaceChildren(...visible.map((i) => h("button", {
      class: "icon-choice", attrs: { type: "button" }, on: { click: safe(async () => {
        dialog.close();
        await find(i, installed.get(i.bundle_id)?.item_id ?? null);
      }) },
    }, h("strong", { text: i.name }), h("small", { text: i.bundle_id }))));
    if (!visible.length) list.append(h("p", { class: "empty", text: "По этому фильтру значков нет." }));
  }
  filter.addEventListener("input", render);
  all.addEventListener("change", render);
  dialog.addEventListener("close", () => { read++; });
  async function open() {
    const phone = targetPhone();
    if (!phone) return;
    const seq = ++read;
    list.replaceChildren();
    filter.value = "";
    all.checked = false;
    filter.disabled = all.disabled = true;
    status.textContent = "Читаем экран Домой и список приложений…";
    dialog.showModal();
    try {
      const [items, apps] = await Promise.all([
        call<HomeIcon[]>("home_screen_apps", { udid: phone.udid }),
        call<InstalledDto[]>("installed_apps", { udid: phone.udid }).catch(() => null),
      ]);
      if (seq !== read || !dialog.open) return;
      icons = items;
      installed = new Map(apps?.map((a) => [a.bundle_id, a]));
      all.disabled = apps === null;
      filter.disabled = false;
      status.textContent = apps === null ? "Список установленных недоступен. Показаны значки экрана Домой." :
        "Показаны значки, которых нет в списке установленных. Выбери приложение для поиска.";
      render();
    } catch (e) {
      if (seq === read) status.textContent = errorText(asUiError(e));
    }
  }
  return { dialog, open };
}
