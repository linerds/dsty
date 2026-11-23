// dsty: проби повної перевірки. Обгортку подають typst через stdin, у
// проєкт нічого не пишеться. Проби не змінюють розкладки: вони додають лише
// невидимі `metadata` і колір тексту, а колір на розміщення не впливає.
//
// Позицій і сторінок проби не читають: `here().position()` на кожному прогоні
// тексту змушує typst робити ще один прохід розкладки й подвоює час. Їх бере
// вираз `typst eval` уже після компіляції (`location().position()`).
//
// Вид вмісту позначається кольором тексту через show-set-правила: вони діють
// і тоді, коли шаблон перебудовує елемент власним show-правилом без `it`.
//
// Секції `// @секція` … `// @end` dsty вирізає, якщо їхні дані не
// потрібні жодному ввімкненому правилу.

#let dsty-kinds = (
  "#010001": "h", // заголовок 2-го рівня й нижче
  "#01000d": "H", // заголовок 1-го рівня
  "#010002": "c", // підпис фігури
  "#010003": "f", // вміст фігури, напис «Продовження таблиці»
  "#010004": "t", // комірки таблиці
  "#010005": "n", // текст виноски
  "#010006": "o", // зміст
  "#010007": "e", // виокремлена формула, її номер
  "#010008": "r", // код
  "#010009": "b", // перелік джерел
  "#01000a": "l", // перелік
  "#01000b": "s", // знак виноски, індекси, формула в рядку
  "#01000e": "z", // надруковане посилання на джерело (`cite`)
)

#let dsty-kind() = {
  let f = text.fill
  if type(f) == color { dsty-kinds.at(f.to-hex(), default: "") } else { "?" }
}

#let dsty-num(v) = if type(v) == length { v.to-absolute().pt() } else if type(v) == relative {
  v.length.to-absolute().pt()
} else { none }

// Початок вмісту як текст: за ним абзац прив'язується до коду. Збирання
// зупиняється після `limit` байтів, бо повний текст довгих абзаців дорогий.
#let dsty-plain(c, limit: 160) = {
  if type(c) == str { return c }
  if type(c) != content { return "" }
  if c.has("text") { return if type(c.text) == str { c.text } else { "" } }
  if c.has("children") {
    let acc = ""
    for x in c.children {
      acc += dsty-plain(x, limit: limit)
      if acc.len() >= limit { break }
    }
    return acc
  }
  if c.has("body") { return dsty-plain(c.body, limit: limit) }
  if c.has("child") { return dsty-plain(c.child, limit: limit) }
  if c.func() == [ ].func() { return " " }
  if c.func() == smartquote { return if c.at("double", default: true) { "\"" } else { "’" } }
  ""
}

// @marks
#show heading: set text(fill: rgb("#010001"))
#show heading.where(level: 1): set text(fill: rgb("#01000d"))
#show figure: set text(fill: rgb("#010003"))
#show figure.caption: set text(fill: rgb("#010002"))
#show table: set text(fill: rgb("#010004"))
#show footnote.entry: set text(fill: rgb("#010005"))
#show outline: set text(fill: rgb("#010006"))
#show math.equation.where(block: true): set text(fill: rgb("#010007"))
#show math.equation.where(block: false): set text(fill: rgb("#01000b"))
#show raw: set text(fill: rgb("#010008"))
#show bibliography: set text(fill: rgb("#010009"))
#show list: set text(fill: rgb("#01000a"))
#show enum: set text(fill: rgb("#01000a"))
#show footnote: set text(fill: rgb("#01000b"))
#show super: set text(fill: rgb("#01000b"))
#show sub: set text(fill: rgb("#01000b"))
#show cite: set text(fill: rgb("#01000e"))
// @end

// @text
// Прогони тексту записуються по одному лише там, де їх так і перевіряють:
// заголовки, зміст, підписи, номери формул і посилань на джерела. Вид відомий з
// кольору-позначки, тож `show text.where(fill: …)` основного тексту не чіпає:
// правило на кожному слові абзацу коштує найбільше в усій обгортці. В
// основному тексті записуються лише збіги ключових слів.

// У підписі записуються слово «Рисунок» і номер
#let dsty-caption-key = regex("^\s*(Рисунок|Таблиця)")
// Номер у підписі («1.2», «А.1»), розділювач « – »
#let dsty-number = regex("^\s*([\dА-ЯA-Z]{1,3}(\.\d+)*\.?|[–—\-:.]|\(.*\)?)\s*$")

#let dsty-run(it, k) = {
  // У підписі звичайний текст без номера й слова «Рисунок» не потрібен
  if k == "c" and it.text.match(dsty-caption-key) == none and it.text.match(dsty-number) == none {
    return
  }
  // Номер формули: ширина для його правого краю
  if k == "e" {
    context [#metadata((k: "run", kind: k, t: it.text, w: measure(it).width.pt()))<dsty-probe>]
  } else {
    [#metadata((k: "run", kind: k, t: it.text))<dsty-probe>]
  }
}

#show text.where(fill: rgb("#010001")): it => { dsty-run(it, "h"); it }
#show text.where(fill: rgb("#01000d")): it => { dsty-run(it, "H"); it }
// Зміст: вміст сторінки після його заголовка (`heading-orphan`)
#show text.where(fill: rgb("#010006")): it => { dsty-run(it, "o"); it }
#show text.where(fill: rgb("#010002")): it => { dsty-run(it, "c"); it }
// Надруковане посилання на джерело: номер стиль загортає в посилання, тож
// «[», «2», «]» стають окремими прогонами, і їх збирає повна перевірка
#show text.where(fill: rgb("#01000e")): it => { dsty-run(it, "z"); it }
// У формулі записується лише номер: прогін з дужками. Числа й рядки тіла формули (їх
// сотні) не записуються, а номер без дужок видно з шаблону нумерації.
#show text.where(fill: rgb("#010007")): it => {
  if it.text.contains("(") or it.text.contains(")") { dsty-run(it, "e") }
  it
}

// Обсяг звіту в рефераті й напис «Продовження таблиці»: запис збігу в
// основному тексті, таблиці й фігурі
#show regex("^\s*(Продовження|Кінець)|\d+\s*(?:с|стор|рис|табл|дод|джерел|посил)\p{L}*\.?"): it => {
  context {
    let k = dsty-kind()
    if k in ("", "t", "f") { [#metadata((k: "run", kind: k, t: it.text))<dsty-probe>] }
  }
  it
}
// @end

// @par
// Абзац: вид і початок тексту, за яким він прив'язується до коду
#show par: it => {
  context {
    let k = dsty-kind()
    if k in ("", "n", "l", "b") {
      [#metadata((k: "par", kind: k, head: dsty-plain(it.body)))<dsty-probe>]
    }
  }
  it
}
// @end

// @table
// Таблиця: межі для переносу між сторінками й наявність головки
#show table: it => {
  let header = it.children.find(c => c.func() == table.header)
  [#metadata((k: "table", header: header != none, repeat: header != none and header.at("repeat", default: true)))<dsty-probe>]
  it
  [#metadata((k: "table-end"))<dsty-probe>]
}
// @end

// @page
// Сторінка: формат і береги. Запис ставиться і на передньому, і на задньому
// плані: документ, що задає власний `foreground` (водяний знак) чи
// `background` (рамку), перекриває лише один з них
#let dsty-page = context {
  let m = page.margin
  let side(name) = if type(m) == dictionary {
    let v = m.at(name, default: m.at(if name in ("left", "right") { "x" } else { "y" }, default: m.at("rest", default: auto)))
    dsty-num(v)
  } else { dsty-num(m) }
  [#metadata((
    k: "page",
    w: page.width.to-absolute().pt(),
    h: page.height.to-absolute().pt(),
    top: side("top"),
    bottom: side("bottom"),
    left: side("left"),
    right: side("right"),
  ))<dsty-probe>]
}
#set page(foreground: dsty-page, background: dsty-page)
// @end

#include "/__MAIN__"
