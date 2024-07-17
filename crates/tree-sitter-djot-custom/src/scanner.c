#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"
#include <stdio.h>

// #define DEBUG

#ifdef DEBUG
#include <assert.h>
#endif

// The different tokens the external scanner support
// See `externals` in `grammar.js` for a description of most of them.
enum TokenType {
  BLANKLINE,
  PARAGRAPH_START,
  PARAGRAPH_END,
  SOFTBREAK,
};

void *tree_sitter_djot_external_scanner_create(void) { return NULL; }

void tree_sitter_djot_external_scanner_destroy(void *payload) {
  // ...
}

unsigned tree_sitter_djot_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  return 0u;
}

void tree_sitter_djot_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  // ...
}

bool tree_sitter_djot_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  // consider two case, start from line beginning or not
  // from begining of a line
  // - blankline
  // - paragraph_start
  // not from begining of a line
  // - softbreak
  // - paragraph_end
  if (lexer->eof(lexer)) {
      return false;
  }

  if (lexer->get_column(lexer) == 0) {
    // jump over leading spaces, treat them as ignored
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      lexer->advance(lexer, true);
    }
    // blankline has highest priority (if we don't consider code block or
    // verbatim)
    if (valid_symbols[BLANKLINE]) {
      if (lexer->lookahead == '\n') {
        lexer->advance(lexer, false);
        lexer->result_symbol = BLANKLINE;
        return true;
      }
    }
    if (valid_symbols[PARAGRAPH_START]) {
      lexer->result_symbol = PARAGRAPH_START;
      return true;
    }
  } else {
    if (valid_symbols[SOFTBREAK] || valid_symbols[PARAGRAPH_END]) {
      if (lexer->lookahead == '\n') {
        lexer->advance(lexer, false);
        lexer->mark_end(lexer);

        if (lexer->eof(lexer)) {
          lexer->result_symbol = PARAGRAPH_END;
          return true;
        }

        // test if it's newline
        while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
          lexer->advance(lexer, false);
        }
        bool is_newline = lexer->lookahead == '\n';
        if (is_newline) {
          lexer->result_symbol = PARAGRAPH_END;
          return true;
        } else {
          lexer->result_symbol = SOFTBREAK;
          return true;
        }
      } else if (lexer->eof(lexer)) {
        lexer->result_symbol = PARAGRAPH_END;
        return true;
      }
    }
  }
  return false;
}
