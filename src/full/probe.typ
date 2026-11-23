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
  "#01000c": "m", // службовий текст вимірювань, не записується
  "#01000e": "z", // надруковане посилання на джерело (`cite`)
)

#let dsty-kind() = {
  let f = text.fill
  if type(f) == color { dsty-kinds.at(f.to-hex(), default: "") } else { "?" }
}

#let dsty-font() = {
  let f = text.font
  lower(if type(f) == array { f.at(0, default: "") } else if type(f) == dictionary { f.name } else { f })
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

// Стиль прогону для запису
#let dsty-style(k) = (
  size: text.size.to-absolute().pt(),
  font: dsty-font(),
  style: text.style,
  black: k != "" or text.fill == black,
)

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
// Прогони тексту записуються по одному лише там, де їх так і перевіряють: заголовки,
// зміст, підписи й номери формул. Вид відомий з кольору-позначки, тож
// `show text.where(fill: …)` основного тексту не чіпає: `context` на кожному
// слові абзацу коштує найбільше в усій обгортці. Стиль основного тексту,
// переліків і виносок записує проба абзаців (@par), а ключові слова, позначки
// переліків і курсив записують правила, що спрацьовують лише на збігах.

// У підписі записуються слово «Рисунок» і номер
#let dsty-caption-key = regex("^\s*(Рисунок|Таблиця)")
// Номер у підписі («1.2», «А.1»), розділювач « – »
#let dsty-number = regex("^\s*([\dА-ЯA-Z]{1,3}(\.\d+)*\.?|[–—\-:.]|\(.*\)?)\s*$")

#let dsty-run(it, k) = context {
  let rec = (k: "run", kind: k, t: it.text) + dsty-style(k)
  // У підписі звичайний текст без номера й слова «Рисунок» не потрібен
  if k == "c" and calc.abs(rec.size - 14) < 0.05 and rec.font == "times new roman" and rec.style == "normal" and it.text.match(dsty-caption-key) == none and it.text.match(dsty-number) == none {
    return
  }
  if k in ("h", "H", "o") {
    rec += (weight: text.weight, hyph: if text.hyphenate == auto { par.justify } else { text.hyphenate })
  }
  if k in ("h", "H", "o", "e") {
    let w = measure(it).width
    rec += (w: w.pt())
    // Великі літери: `upper()` шаблону не змінює `it.text`, тож порівнюємо ширину
    if k == "H" { rec += (upper: w == measure(upper(it)).width and w != measure(lower(it)).width) }
  }
  [#metadata(rec)<dsty-probe>]
}

#show text.where(fill: rgb("#010001")): it => { dsty-run(it, "h"); it }
#show text.where(fill: rgb("#01000d")): it => { dsty-run(it, "H"); it }
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

// Збіг у тексті видів `kinds`: запис прогону з самим збігом
#let dsty-match(it, kinds) = context {
  let k = dsty-kind()
  if k in kinds { [#metadata((k: "run", kind: k, t: it.text) + dsty-style(k))<dsty-probe>] }
}
// Обсяг звіту в рефераті й напис «Продовження таблиці»
#show regex("^\s*(Продовження|Кінець)|\d+\s*(?:с|стор|рис|табл|дод|джерел|посил)\p{L}*\.?"): it => {
  dsty-match(it, ("", "t", "f"))
  it
}
// Надрукована позначка позиції переліку («–», «а)», «1)»): окремий короткий текст
#show regex("^\s*(\S{1,4}[.)]|[–—\-•‣∙◦*])\s*$"): it => {
  dsty-match(it, ("l",))
  it
}
// Курсив в основному тексті й переліках
#show text.where(style: "italic"): it => {
  dsty-match(it, ("", "l"))
  it
}
// @end

// @par
// Абзац: кегль, відступ, інтервал (крок рядків вимірюється на службовому
// тексті; `measure` з тими самими стилями typst кешує).
#show par: it => {
  context {
    let k = dsty-kind()
    if k in ("", "n", "l", "b") {
      let probe(body) = measure(text(fill: rgb("#01000c"), body)).height
      // Поле самого абзацу: враховує й `#par(first-line-indent: …)[…]`
      let indent = it.at("first-line-indent", default: par.first-line-indent)
      let (amount, all) = if type(indent) == dictionary { (indent.at("amount", default: 0pt), indent.at("all", default: false)) } else { (indent, false) }
      // `\` на початку: перший рядок порожній, текст іде з другого, без відступу
      let lead = if it.body.has("children") {
        it.body.children.find(c => c.func() != [ ].func())
      } else { it.body }
      [#metadata((
        k: "par",
        kind: k,
        size: text.size.to-absolute().pt(),
        font: dsty-font(),
        style: text.style,
        black: k != "" or text.fill == black,
        indent: amount.to-absolute().pt(),
        all: all,
        lead-break: lead != none and lead.func() == linebreak,
        pitch: (probe[Ж#linebreak()Ж] - probe[Ж]).pt(),
        head: dsty-plain(it.body),
      ))<dsty-probe>]
    }
  }
  it
}
// Слово «де» в основному тексті: де його надруковано насправді. Відступ
// рядка пояснень під формулою можна зняти й без `first-line-indent`
// (`#h(-1.25cm)`, `\`), тож лише налаштувань замало.
#show regex("\bде\b"): it => {
  context if dsty-kind() == "" { [#metadata((k: "run", kind: "w", t: it.text) + dsty-style(""))<dsty-probe>] }
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
// Сторінка: формат, береги, номер, роздільник виносок. Запис ставиться і на передньому,
// і на задньому плані: документ, що задає власний `foreground` (водяний знак)
// чи `background` (рамку), перекриває лише один з них
#let dsty-page = context {
  let m = page.margin
  let side(name) = if type(m) == dictionary {
    let v = m.at(name, default: m.at(if name in ("left", "right") { "x" } else { "y" }, default: m.at("rest", default: auto)))
    dsty-num(v)
  } else { dsty-num(m) }
  let n = if page.numbering == none { none } else {
    let r = if type(page.numbering) == str { numbering(page.numbering, ..counter(page).get()) } else {
      (page.numbering)(..counter(page).get())
    }
    if r == none { none } else { dsty-plain(r) }
  }
  let sep = footnote.entry.separator
  let len = if type(sep) == content and sep.func() == line { sep.at("length", default: none) }
  let sep = if type(len) == length { (len.to-absolute().pt(), 0) } else if type(len) == relative {
    (len.length.to-absolute().pt(), len.ratio / 100%)
  } else if type(len) == ratio { (0, len / 100%) }
  [#metadata((
    k: "page",
    w: page.width.to-absolute().pt(),
    h: page.height.to-absolute().pt(),
    top: side("top"),
    bottom: side("bottom"),
    left: side("left"),
    right: side("right"),
    num: n,
    numbered: page.numbering != none,
    align: repr(page.number-align),
    header: page.header == auto,
    sep: sep,
  ))<dsty-probe>]
}
#set page(foreground: dsty-page, background: dsty-page)
// @end

#include "/__MAIN__"
