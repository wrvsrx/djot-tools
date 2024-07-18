module.exports = grammar({
  name: "djot",

  extras: ($) => ["\r", $._ignored,],

  rules: {
    document: ($) => repeat($.block),

    block: ($) => choice($.paragraph, $.blankline, $.heading),

    paragraph: ($) => seq($._block_like_start, repeat1($.inline), $._block_like_end),
    blankline: ($) => seq($._block_like_start, $._block_like_end),
    heading: ($) => seq($._block_like_start, repeat1(seq($.heading_marker, repeat($.inline))), $._block_like_end),
    inline: ($) => choice($.str, $.softbreak),
    str: (_) => /.+/,
  },

  externals: ($) => [
    // block level
    // zero-length token
    $._block_like_start,
    // zero-length token or '\n'
    $._block_like_end,

    // block leading
    $.heading_marker,

    // inline level
    $.softbreak,

    $._ignored,
  ],
});
