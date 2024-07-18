#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"

// #define TREE_SITTER_DEBUG

#ifdef TREE_SITTER_DEBUG
#include <assert.h>
#include <stdio.h>
#endif

// The different tokens the external scanner support
// See `externals` in `grammar.js` for a description of most of them.
enum TokenType {
  BLOCK_LIKE_START,
  BLOCK_LIKE_END_EOL,
  BLOCK_LIKE_END_ZERO_LENGTH,

  HEADING_MARKER,

  SOFTBREAK,

  IGNORED,
};

enum BlockLikeType {
  PARAGRAPH,
  BLANKLINE,
  HEADING,
  SECTION,
};

struct Empty {};

struct HeadingState {
  uint8_t level;
  // we only parse marker at line start (allow leading spaces)
  bool need_to_parse;
};

struct SectionState {
  uint8_t level;
};

union BlockLikeMetadata {
  struct Empty paragraph;
  struct Empty blankline;
  struct HeadingState heading;
  struct SectionState section;
};

struct BlockLike {
  enum BlockLikeType type;
  union BlockLikeMetadata data;
};
typedef Array(struct BlockLike) BlockLikeStack;

// we need to track line_parsing_state since there're zero length token
// we can only differentiate them by line_parsing_state
// starting_marker paired
// parsing_block_list_start -> parsing_starting_marker
//   if ignored, -> otherwise
//   otherwise -> parsing_block_list_start
enum LineParsingState {
  PARSING_BLOCK_LIKE_START,
  PARSING_STARTING_MARKER,
  OTHERWISE,
  PARSING_BLOCK_LIKE_END_ZERO_LENGTH,
};

struct ScannerState {
  // have we parse the line start
  // use this flag to avoid stuck at empty line
  // this flag is reset every time we consume '\n'
  enum LineParsingState line_parsing_state;
  // store all open blocks
  BlockLikeStack block_like_stack;
};

void init(struct ScannerState *s) {
  array_init(&(s->block_like_stack));
  s->line_parsing_state = PARSING_BLOCK_LIKE_START;
}
void *tree_sitter_djot_external_scanner_create(void) {
  struct ScannerState *s = ts_malloc(sizeof(struct ScannerState));
  init(s);
  return s;
}

void tree_sitter_djot_external_scanner_destroy(void *payload) {
  struct ScannerState *s = payload;
  ts_free(s);
}

#define SAVE_TO_BUFFER(buffer, size, value)                                    \
  *(__typeof__(value) *)(buffer + size) = value;                               \
  size += sizeof(__typeof__(value));

#define LOAD_FROM_BUFFER(buffer, size, value)                                  \
  value = *(__typeof__(value) *)(buffer + size);                               \
  size += sizeof(__typeof__(value));

unsigned tree_sitter_djot_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  struct ScannerState *s = payload;
  unsigned size = 0;
  SAVE_TO_BUFFER(buffer, size, s->line_parsing_state);
  SAVE_TO_BUFFER(buffer, size, s->block_like_stack.size);
  for (__typeof__(s->block_like_stack.size) i = 0; i < s->block_like_stack.size;
       ++i) {
    SAVE_TO_BUFFER(buffer, size, s->block_like_stack.contents[i]);
  }
  return size;
}

void tree_sitter_djot_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  struct ScannerState *s = payload;
  init(s);

  if (length == 0) {
    return;
  }

  unsigned size = 0;
  LOAD_FROM_BUFFER(buffer, size, s->line_parsing_state);
  __typeof__(s->block_like_stack.size) count;
  LOAD_FROM_BUFFER(buffer, size, count);

  array_grow_by(&(s->block_like_stack), count);
  for (__typeof__(count) i = 0; i < count; ++i) {
    LOAD_FROM_BUFFER(buffer, size, s->block_like_stack.contents[i]);
  }
  assert(length == size);
}

static void consume_whitespace(TSLexer *lexer) {
#ifdef TREE_SITTER_DEBUG
  printf("--- consume whitespace\n");
#endif
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
  }
}

static void push_block_like(struct ScannerState *s, struct BlockLike b) {
#ifdef TREE_SITTER_DEBUG
  if (b.type == PARAGRAPH) {
    printf("--- push paragraph\n");
  } else if (b.type == BLANKLINE) {
    printf("--- push blankline\n");
  } else if (b.type == HEADING) {
    printf("--- push heading %d\n", b.data.heading.level);
  } else if (b.type == SECTION) {
    printf("--- push section %d\n", b.data.section.level);
  } else {
    assert(false);
  }
#endif
  array_push(&(s->block_like_stack), b);
}

static void pop_block_like(struct ScannerState *s) {
  struct BlockLike t = *array_back(&(s->block_like_stack));
#ifdef TREE_SITTER_DEBUG
  if (t.type == PARAGRAPH) {
    printf("---pop paragraph\n");
  } else if (t.type == BLANKLINE) {
    printf("---pop blankline\n");
  } else if (t.type == HEADING) {
    printf("---pop heading %d\n", t.data.heading.level);
  } else if (t.type == SECTION) {
    printf("---pop section %d\n", t.data.section.level);
  } else {
    assert(false);
  }
#endif
  array_pop(&(s->block_like_stack));
}

static void accept_block_like_end_eol(struct ScannerState *s, TSLexer *lexer,
                                      const bool *valid_symbols) {
  assert(valid_symbols[BLOCK_LIKE_END_EOL]);
  lexer->result_symbol = BLOCK_LIKE_END_EOL;
  pop_block_like(s);
}
static void accept_block_like_end_zero_length(struct ScannerState *s,
                                              TSLexer *lexer,
                                              const bool *valid_symbols) {
  assert(valid_symbols[BLOCK_LIKE_END_ZERO_LENGTH]);
  lexer->result_symbol = BLOCK_LIKE_END_ZERO_LENGTH;
  pop_block_like(s);
}

static void accept_softbreak(struct ScannerState *s, TSLexer *lexer,
                             const bool *valid_symbols) {
#ifdef TREE_SITTER_DEBUG
  printf("--- accept softbreak\n");
#endif
  assert(valid_symbols[SOFTBREAK]);
  lexer->result_symbol = SOFTBREAK;
}

static void accept_ignored(struct ScannerState *s, TSLexer *lexer,
                           const bool *valid_symbols) {
#ifdef TREE_SITTER_DEBUG
  printf("--- accept ignored\n");
#endif
  assert(valid_symbols[IGNORED]);
  lexer->result_symbol = IGNORED;
}

static uint8_t count_heading_level(TSLexer *lexer) {
  uint8_t heading_level = 0;
  while (lexer->lookahead == '#') {
    ++heading_level;
    lexer->advance(lexer, false);
  }
#ifdef TREE_SITTER_DEBUG
  printf("--- heading level %d\n", heading_level);
#endif
  return heading_level;
}

static bool parse_eol(struct ScannerState *s, TSLexer *lexer,
                      const bool *valid_symbols) {
  // if it's eol
  // we must accpet that since we always parse eol manually
  assert(s->block_like_stack.size > 0);
  struct BlockLike *t = array_back(&(s->block_like_stack));
  if (t->type == BLANKLINE) {
    accept_block_like_end_eol(s, lexer, valid_symbols);
  } else if (t->type == PARAGRAPH) {
    if (lexer->eof(lexer)) {
      accept_block_like_end_eol(s, lexer, valid_symbols);
    } else {
      consume_whitespace(lexer);
      bool is_newline = lexer->lookahead == '\n';
      if (is_newline) {
        accept_block_like_end_eol(s, lexer, valid_symbols);
      } else {
        accept_softbreak(s, lexer, valid_symbols);
      }
    }
  } else if (t->type == HEADING) {
    // reset need_to_parse flag
    t->data.heading.need_to_parse = true;
    if (lexer->eof(lexer)) {
      accept_block_like_end_eol(s, lexer, valid_symbols);
    } else {
      consume_whitespace(lexer);
      if (lexer->lookahead == '\n') {
        accept_block_like_end_eol(s, lexer, valid_symbols);
      } else if (lexer->lookahead == '#') {
        uint8_t heading_level = count_heading_level(lexer);
        if (lexer->lookahead == ' ' || lexer->lookahead == '\n') {
          if (heading_level == t->data.heading.level) {
            accept_softbreak(s, lexer, valid_symbols);
          } else {
            accept_block_like_end_eol(s, lexer, valid_symbols);
          }
        } else {
          accept_softbreak(s, lexer, valid_symbols);
        }
      } else {
        accept_softbreak(s, lexer, valid_symbols);
      }
    }
  } else if (t->type == SECTION) {
    // it won't happen since section contains at least one block, which will
    // consume the '\n'
    assert(false);
  } else {
    assert(false);
  }
  return true;
}

static bool parse_block_like_start(struct ScannerState *s, TSLexer *lexer,
                                   const bool *valid_symbols) {
  // suppose we have closed blocks properly when parsing eol
  consume_whitespace(lexer);
  // this parser doesn't consume any token
  lexer->mark_end(lexer);
#ifdef TREE_SITTER_DEBUG
  printf("--- parsing block_like_start\n");
#endif
  if (s->block_like_stack.size == 0) {
#ifdef TREE_SITTER_DEBUG
    printf("--- no block is open\n");
#endif
    // if no block is open, search for block start
    if (lexer->lookahead == '\n') {
      struct BlockLike b = {.type = BLANKLINE, .data = {.blankline = {}}};
      push_block_like(s, b);
    } else if (lexer->lookahead == '#') {
      // if we fount a # at outmost scope, we create a section

      // we just count for # number, don't consume them
      lexer->mark_end(lexer);
      uint8_t heading_level = count_heading_level(lexer);
      if (lexer->lookahead == ' ' || lexer->lookahead == '\n') {
        struct BlockLike b = {.type = SECTION,
                              .data = {.section = {.level = heading_level}}};
        push_block_like(s, b);
      } else {
        struct BlockLike b = {.type = PARAGRAPH, .data = {.paragraph = {}}};
        push_block_like(s, b);
      }
    } else {
      struct BlockLike b = {.type = PARAGRAPH, .data = {.paragraph = {}}};
      push_block_like(s, b);
    }
    assert(valid_symbols[BLOCK_LIKE_START]);
    lexer->result_symbol = BLOCK_LIKE_START;
  } else {
    struct BlockLike *t = array_back(&(s->block_like_stack));
    if (t->type == PARAGRAPH) {
      // if current block is paragraph, continue parsing since it can't nest
      // other blocks
      accept_ignored(s, lexer, valid_symbols);
    } else if (t->type == BLANKLINE) {
      // if current block is blankline, it's impossible
      assert(false);
    } else if (t->type == HEADING) {
      // if current block is paragraph, continue parsing since it can't nest
      // other blocks
      accept_ignored(s, lexer, valid_symbols);
    } else if (t->type == SECTION) {
      // there might be two cases
      // 1. we meet another blocks
      // 2. we meet the first heading
      // 3. we meet heading in following blocks

      if (lexer->lookahead == '\n') {
        // 1. we meet another blocks
        struct BlockLike b = {.type = BLANKLINE, .data = {.blankline = {}}};
        push_block_like(s, b);
      } else if (lexer->lookahead == '#') {
        uint8_t heading_level = count_heading_level(lexer);
#ifdef TREE_SITTER_DEBUG
        printf("--- heading level %d, section level: %d\n", heading_level,
               t->data.section.level);
#endif
        if (heading_level == t->data.section.level) {
          // 2. we meet the first heading
          // we can make such assumption since we have closed blocks properly
          struct BlockLike b = {.type = HEADING,
                                .data = {.heading = {.level = heading_level,
                                                     .need_to_parse = true}}};
          push_block_like(s, b);
        } else if (heading_level > t->data.section.level) {
          // 3. we meet heading in following blocks
          struct BlockLike b = {.type = SECTION,
                                .data = {.section = {.level = heading_level}}};
          push_block_like(s, b);
        } else {
          // we can make such assumption since we have closed blocks properly
          assert(false);
        }
      } else {
        struct BlockLike b = {.type = PARAGRAPH, .data = {.paragraph = {}}};
        push_block_like(s, b);
      }
      assert(valid_symbols[BLOCK_LIKE_START]);
      lexer->result_symbol = BLOCK_LIKE_START;
    } else {
      assert(false);
    }
  }
  return true;
}

static bool parse_starting_maker(struct ScannerState *s, TSLexer *lexer,
                                 const bool *valid_symbols) {
  assert(s->block_like_stack.size > 0);
  struct BlockLike *t = array_back(&(s->block_like_stack));
  if (t->type == HEADING && t->data.heading.need_to_parse) {
    // we parse heading marker only once each line
    t->data.heading.need_to_parse = false;
    if (lexer->lookahead == '#') {
      // if current block is heading, there is two possible cases
      // 1. it's a heading marker
      // 2. it's simple continuation
      // we just need to handle the first case, the latter can be handled by
      // default scanner
      uint8_t heading_level = count_heading_level(lexer);
      if (lexer->lookahead == ' ' || lexer->lookahead == '\n') {
        assert(heading_level == t->data.heading.level);
        assert(valid_symbols[HEADING_MARKER]);
        lexer->result_symbol = HEADING_MARKER;
      } else {
        accept_ignored(s, lexer, valid_symbols);
      }
    } else {
      accept_ignored(s, lexer, valid_symbols);
    }
  } else {
    accept_ignored(s, lexer, valid_symbols);
  }
  return true;
}

static bool parse_block_like_end_zero_length(struct ScannerState *s,
                                             TSLexer *lexer,
                                             const bool *valid_symbols) {
  lexer->mark_end(lexer);
  if (s->block_like_stack.size == 0) {
    // if no block is open, we don't need to close anything
    accept_ignored(s, lexer, valid_symbols);
    s->line_parsing_state = PARSING_BLOCK_LIKE_START;
    return true;
  }

  struct BlockLike *t = array_back(&(s->block_like_stack));
  if (t->type == PARAGRAPH) {
    // it's impossible since PARAGRAPH can't nest other blocks
    assert(false);
  } else if (t->type == BLANKLINE) {
    // it's impossible since BLANKLINE can't nest other blocks
    assert(false);
  } else if (t->type == HEADING) {
    // it's impossible since HEADING can't nest other blocks
    assert(false);
  } else if (t->type == SECTION) {

    if (lexer->eof(lexer)) {
      accept_block_like_end_zero_length(s, lexer, valid_symbols);
      return true;
    }
    consume_whitespace(lexer);

    if (lexer->lookahead == '#') {
      uint8_t heading_level = count_heading_level(lexer);
      if (heading_level <= t->data.section.level) {
        // if next line is heading at the same or lower level, close this
        // section
        accept_block_like_end_zero_length(s, lexer, valid_symbols);
      } else {
        accept_ignored(s, lexer, valid_symbols);
      }
    } else {
      accept_ignored(s, lexer, valid_symbols);
    }
  } else {
    assert(false);
  }
  if (lexer->result_symbol == IGNORED) {
    s->line_parsing_state = PARSING_BLOCK_LIKE_START;
  } else {
    s->line_parsing_state = PARSING_BLOCK_LIKE_END_ZERO_LENGTH;
  }
  return true;
}

bool tree_sitter_djot_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  struct ScannerState *s = payload;

  if (s->line_parsing_state == PARSING_BLOCK_LIKE_START) {
#ifdef TREE_SITTER_DEBUG
    printf("--- parsing block_like_start\n");
#endif
    if (lexer->eof(lexer)) {
      return false;
    }
    assert(parse_block_like_start(s, lexer, valid_symbols));
    struct BlockLike *t = array_back(&(s->block_like_stack));
    if (t->type == HEADING) {
      // we only need to parse starting marker when
      // - HEADING
      s->line_parsing_state = PARSING_STARTING_MARKER;
    } else if (t->type == PARAGRAPH || t->type == BLANKLINE) {
      // we don't need to parse starting marker when
      // - PARAGRAPH
      // - BLANKLINE
      s->line_parsing_state = OTHERWISE;
    } else if (t->type == SECTION) {
      // we have to determine inner block type
      s->line_parsing_state = PARSING_BLOCK_LIKE_START;
    } else {
      assert(false);
    }
    return true;
  } else if (s->line_parsing_state == PARSING_STARTING_MARKER) {
    assert(parse_starting_maker(s, lexer, valid_symbols));
    if (lexer->result_symbol == IGNORED) {
      s->line_parsing_state = OTHERWISE;
    } else {
      s->line_parsing_state = PARSING_BLOCK_LIKE_START;
    }
    return true;
  } else if (s->line_parsing_state == OTHERWISE) {
    if (lexer->lookahead == '\n') {
      lexer->advance(lexer, false);
      lexer->mark_end(lexer);
      // parse softbreak or block_like_end
      // if it isn't start or start has been parsed
      assert(parse_eol(s, lexer, valid_symbols));
      if (lexer->result_symbol == BLOCK_LIKE_END_EOL) {
        s->line_parsing_state = PARSING_BLOCK_LIKE_END_ZERO_LENGTH;
      } else if (lexer->result_symbol == SOFTBREAK) {
        s->line_parsing_state = PARSING_BLOCK_LIKE_START;
      } else {
        assert(false);
      }
      return true;
    }
  } else if (s->line_parsing_state == PARSING_BLOCK_LIKE_END_ZERO_LENGTH) {
#ifdef TREE_SITTER_DEBUG
    printf("--- parsing block_like_end_zero_length\n");
#endif
    assert(parse_block_like_end_zero_length(s, lexer, valid_symbols));
    return true;
  } else {
    assert(false);
  }

  return false;
}
