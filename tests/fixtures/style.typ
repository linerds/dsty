// Оформлення мінішаблону для тестів: основний текст і додатки за ДСТУ.
#let letters = "АБВГДЕЖИКЛМНПРСТУФХЦШЩЮЯ".clusters()

#let dstu(it) = {
  set page(
    paper: "a4",
    margin: (top: 20mm, right: 10mm, bottom: 20mm, left: 25mm),
    numbering: (i, ..) => if i > 1 { numbering("1", i) },
    number-align: top + right,
    // Виноски нумерують у межах кожної сторінки
    background: counter(footnote).update(0),
  )
  set text(lang: "uk", size: 14pt, hyphenate: false, font: ("Times New Roman", "Liberation Serif"))
  set par(justify: true, leading: 0.95em, first-line-indent: (amount: 1.25cm, all: true))
  set heading(numbering: "1.1")
  set list(indent: 1.25cm, marker: [--])
  set enum(indent: 1.25cm, numbering: "1)")
  set footnote(numbering: "1)")
  set footnote.entry(separator: line(length: 35mm))
  show footnote.entry: set text(size: 12pt)
  set figure.caption(separator: [ -- ])
  show figure.where(kind: table): set figure.caption(position: top)
  show heading: it => {
    set text(size: 14pt)
    if it.level == 1 {
      pagebreak(weak: true)
      upper(it)
    } else {
      pad(left: 1.25cm, it)
    }
  }
  it
}

#let appendices(it) = {
  counter(heading).update(0)
  set heading(numbering: (i, ..n) => letters.at(i - 1) + numbering(".1.1", ..n))
  set heading(supplement: [Додаток])
  show heading.where(level: 1): it => {
    pagebreak(weak: true)
    align(center)[*ДОДАТОК #counter(heading).display()* \ #it.body]
  }
  it
}
