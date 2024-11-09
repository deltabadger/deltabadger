class User::Name
  PATTERN = '^(?<=^|\s)[\p{L} ]+(\s+[\p{L} ]+)*(?=\s|$)$'.freeze
end
