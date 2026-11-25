{
  // Вираз для `typst eval --in -`: виконується вже після компіляції, тож
  // позиції тут нічого не коштують. Повертає один впорядкований потік
  // елементів документа й записів проб.
  let plain(c, limit: none) = {
    if type(c) == str { return c }
    if type(c) != content { return "" }
    if c.has("text") { return if type(c.text) == str { c.text } else { "" } }
    if c.has("children") {
      let acc = ""
      for x in c.children {
        acc += plain(x, limit: limit)
        if limit != none and acc.len() >= limit { break }
      }
      return acc
    }
    if c.has("body") { return plain(c.body, limit: limit) }
    if c.has("child") { return plain(c.child, limit: limit) }
    if c.func() == [ ].func() { return " " }
    if c.func() == smartquote { return if c.at("double", default: true) { "\"" } else { "’" } }
    ""
  }
  let head(c) = plain(c, limit: 80)
  let lab(e) = if e.has("label") { str(e.label) } else { none }
  let num(n) = if n == none { none } else if type(n) == str { n } else { "fn" }
  let sel = selector(heading).or(figure).or(math.equation).or(footnote).or(ref).or(list).or(enum).or(cite).or(raw.where(block: true)).or(image).or(<dsty-probe>)
  query(sel).map(e => {
    let p = e.location().position()
    let at = (pg: p.page, x: p.x.pt(), y: p.y.pt())
    let f = e.func()
    if f == metadata { at + e.value } else if f == heading {
      at + (
        k: "heading",
        level: e.level,
        num: num(e.numbering),
        body: plain(e.body),
      )
    } else if f == figure {
      at + (
        k: "figure",
        kind: repr(e.kind),
        num: num(e.numbering),
        label: lab(e),
        floating: e.at("placement", default: none) != none,
        caption: if e.caption == none { none } else { plain(e.caption.body) },
      )
    } else if f == math.equation {
      if e.block { at + (k: "equation", num: num(e.numbering), label: lab(e)) }
    } else if f == footnote {
      at + (
        k: "footnote",
        again: type(e.body) == label,
        head: if type(e.body) == label { none } else { head(e.body) },
      )
    } else if f == ref {
      at + (k: "ref", target: str(e.target))
    } else if f == list {
      at + (k: "list", head: if e.children.len() > 0 { head(e.children.at(0).body) } else { "" })
    } else if f == cite {
      at + (k: "cite", key: str(e.key))
    } else if f == enum {
      at + (k: "enum", head: if e.children.len() > 0 { head(e.children.at(0).body) } else { "" })
    } else if f == raw {
      at + (k: "raw")
    } else if f == image {
      at + (k: "image")
    }
  }).filter(x => x != none)
}
