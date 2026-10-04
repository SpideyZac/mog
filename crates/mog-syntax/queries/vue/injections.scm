; <style> is css
((style_element
  (raw_text) @injection.content)
 (#set! injection.language "css"))

; <script lang="ts"> and friends name their language
((script_element
  (start_tag
    (attribute
      (attribute_name) @_attr
      (quoted_attribute_value
        (attribute_value) @injection.language)))
  (raw_text) @injection.content)
 (#eq? @_attr "lang"))

; a plain <script> is javascript
((script_element
  (start_tag) @_tag
  (raw_text) @injection.content)
 (#not-match? @_tag "lang=")
 (#set! injection.language "javascript"))

; {{ expressions }} are javascript
((interpolation
  (raw_text) @injection.content)
 (#set! injection.language "javascript"))
