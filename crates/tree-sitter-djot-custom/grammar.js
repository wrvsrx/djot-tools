module.exports = grammar({
  name: "djot",

  extras: ($) => [$._ignored],

  rules: {
    document: ($) => repeat($.block),

    block: ($) => choice($.section, $.heading, $.paragraph, $.blankline),

    section: ($) => seq($._section_start, $.heading, repeat($.block), $._section_end),
    heading: ($) => seq($._heading_start, $.heading_marker, repeat(choice($.heading_marker, $.inline)), $._heading_end),
    paragraph: ($) => seq($._paragraph_start, repeat1($.inline), $._paragraph_end),
    blankline: ($) => seq($._blankline_start, $._blankline_end),

    inline: ($) => choice($.str, $.softbreak, $.emphasis),
    str: ($) => seq($._str_start, repeat1($._word), $._str_end),
    emphasis: ($) => seq($._emphasis_start, repeat($.inline), $._emphasis_end),
  },

  externals: ($) => [
    $._section_start,
    $._section_end,

    $._heading_start,
    $.heading_marker,
    $._heading_end,

    $._paragraph_start,
    $._paragraph_end,

    $._blankline_start,
    $._blankline_end,

    $._str_start,
    $._str_end,
    $._word,
    $.softbreak,

    $._emphasis_start,
    $._emphasis_end,

    $._ignored,
  ],
});
