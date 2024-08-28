data BlockState = ParagraphState InlineState
data InlineState = StringState

-- 记录当前 parse 的状态
data State = Maybe BlockState

data TokenType = StrToken | NewlineToken
data Token = Token {tokenType :: TokenType, length :: Int}

parse :: State -> String -> (State, Token)
parse = undefined

-- 每次从 state 这里获得一个 parser，
-- 如果当前在 parse block close 状态
-- 从上到下判断每个 Block 是否需要结束
-- 如果不需要结束，emit 一个 softbreak 继续 parse inline
-- 如果需要结束，继续 parse block close 状态

-- 如果当前在 parse block start 状态
-- 尝试开启当前可行的 block，如果开启成功，继续 parse block；否则，转到 parse inline 状态

-- 状态应该可以存在 stack 上面。

parseEol :: State -> String -> (State, [Token])
parseEol s xs = undefined

parseInline :: State -> String -> (State, [Token])
parseInline s (x : xs) = case x of
  '\n' -> parseEol s (x : xs)
  _ -> undefined

