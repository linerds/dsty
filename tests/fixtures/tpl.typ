// Мінішаблон для тестів: обгортки (рисунок, таблиця, перелік джерел,
// реферат) і рецепт документа. dsty не знає жодного шаблону й розпізнає їх
// лише за тим, що вони роблять.
#import "/style.typ"

#let bold(body) = text(weight: "bold", body)

/// Ненумерований заголовок першого рівня.
#let nheading(title) = heading(depth: 1, numbering: none, title)

#let img-caption(caption, source) = if source == none {
  caption + " (рисунок виконано самостійно)"
} else {
  caption
}

#let fig(file, caption, ..args) = {
  if type(file) == str { panic("fig: pass path(…)") }
  let source = args.pos().at(0, default: none)
  let named = args.named()
  let lbl = named.remove("label", default: none)
  [#figure(image(file, ..named), caption: img-caption(caption, source)) #lbl]
}

#let tab(caption: none, columns: none, header: none, label: none, ..args) = {
  if caption == none { panic("tab: caption is required") }
  if type(header) != array { panic("tab: header must be an array") }
  [#figure(
      table(columns: columns, ..args.named(), table.header(..header), ..args.pos()),
      caption: caption,
    ) #label]
}

#let bib(source, title: [Перелік джерел посилання], full: false) = {
  show block: it => it.body
  bibliography(source, title: title, full: full)
}

#let code(source) = raw(source, block: true)

#let abstract-page(w, keywords: (), body) = [
  #align(center, bold[РЕФЕРАТ]) \
  Звіт: #keywords.len() ключових слів.

  #keywords.map(upper).join(", ")

  #body
]
#let abstract = abstract-page

/// Рецепт: шаблон, реферат, вміст, перелік джерел, додатки.
#let coursework(doc, abstract: none, bib-path: none, appendices: none) = {
  show: style.dstu
  if abstract != none {
    abstract-page((:), keywords: abstract.at("keywords", default: ()), abstract.at("text", default: []))
  }
  doc
  if bib-path != none { bib(bib-path) }
  if appendices != none {
    show: style.appendices
    appendices
  }
}
