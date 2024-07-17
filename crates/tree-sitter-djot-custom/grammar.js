module.exports = grammar({
  name: "djot",

  extras: (_) => ["\r"],

  rules: {
    document: ($) => repeat($.block),

    // block must start with zero or more leading spaces
    // block must end with '\n'
    block: ($) => choice($.paragraph, $.blankline),

    paragraph: ($) => seq($.paragraph_start, repeat1($.inline), $.paragraph_end),
    inline: ($) => choice($.str, $.softbreak),
    str: (_) => /.+/,
    paragraph_end: (_) => /\n/,
  },

  externals: ($) => [
    // block level
    $.blankline,
    $.paragraph_start,
    $.paragraph_end,

    // inline level
    $.softbreak,
  ],
});
