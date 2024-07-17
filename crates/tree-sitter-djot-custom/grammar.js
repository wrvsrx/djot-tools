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
    str: ($) => /.+/,
    paragraph_end: ($) => /\n/,
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
