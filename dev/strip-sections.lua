-- Pandoc Lua filter: unwrap djot's implicit heading <section> divs so the
-- generated Markdown is plain headings, not nested <div class="section"> noise.
function Div(el)
  if el.classes:includes('section') then
    return el.content
  end
end
