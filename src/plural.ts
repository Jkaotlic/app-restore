/** Склонение «приложение / приложения / приложений» по числу. */
export function appsWord(n: number): string {
  const d10 = n % 10;
  const d100 = n % 100;
  if (d10 === 1 && d100 !== 11) return "приложение";
  if (d10 >= 2 && d10 <= 4 && (d100 < 12 || d100 > 14)) return "приложения";
  return "приложений";
}

/** Надпись главной кнопки экрана «Выбор». */
export function returnLabel(n: number): string {
  return n > 0 ? `Вернуть ${n} ${appsWord(n)}` : "Выберите приложения";
}
