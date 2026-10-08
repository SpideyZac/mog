; html is parsed with the vue grammar, see the note in lib.rs

(comment) @comment

(tag_name) @tag
(erroneous_end_tag_name) @tag

(attribute_name) @attribute
(attribute_value) @string
(quoted_attribute_value) @string

["<" ">" "</" "/>"] @punctuation.bracket
["="] @operator
