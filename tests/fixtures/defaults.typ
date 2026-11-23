// Типові значення typst, з якими рушій dsty відтворює оформлення
// (`src/defaults.rs`): тест `defaults_match_typst` читає їх через `typst query`.
#let d(k, v) = [#metadata((k: k, v: repr(v)))<d>]
#context {
  d("text.size", text.size)
  d("text.font", text.font)
  d("text.weight", text.weight)
  d("text.lang", text.lang)
  d("par.leading", par.leading)
  d("page.width", page.width)
  d("page.height", page.height)
  d("page.number-align", page.number-align)
  d("list.marker", list.marker)
  d("enum.numbering", enum.numbering)
  d("footnote.numbering", footnote.numbering)
  d("footnote.entry.separator", footnote.entry.separator)
  d("figure.caption.position", figure.caption.position)
  d("math.equation.number-align", math.equation.number-align)
}
#for l in range(1, 4) {
  heading(level: l)[#context {
    d("heading" + str(l) + ".size", text.size)
    d("heading" + str(l) + ".weight", text.weight)
  }]
}
Текст#footnote[#context {
  d("footnote.size", text.size)
  d("footnote.leading", par.leading)
}]
#for lang in ("uk", "en", "ru") {
  set text(lang: lang)
  show figure.caption: it => {
    context d("separator." + lang + "." + repr(it.kind), it.separator)
    it
  }
  show figure: it => {
    d("supplement." + lang + "." + repr(it.kind), it.supplement)
    it
  }
  figure(rect(), caption: [a])
  figure(table([a]), caption: [b])
  figure(raw("x"), caption: [c])
}
