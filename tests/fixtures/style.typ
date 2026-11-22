// Оформлення мінішаблону для тестів: основний текст і додатки.
#let letters = "АБВГДЕЖИКЛМНПРСТУФХЦШЩЮЯ".clusters()

#let dstu(it) = {
  set page(paper: "a4", margin: (top: 20mm, right: 10mm, bottom: 20mm, left: 25mm))
  set text(lang: "uk", size: 14pt, font: ("Times New Roman", "Liberation Serif"))
  set par(justify: true, leading: 0.95em, first-line-indent: (amount: 1.25cm, all: true))
  set heading(numbering: "1.1")
  show heading: it => {
    set text(size: 14pt)
    it
  }
  it
}

#let appendices(it) = {
  counter(heading).update(0)
  set heading(numbering: (i, ..n) => letters.at(i - 1) + numbering(".1.1", ..n))
  set heading(supplement: [Додаток])
  it
}
