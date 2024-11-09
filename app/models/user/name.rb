class User::Name
  def self.pattern
    '^(?<=^|\s)[\p{L} ]+(\s+[\p{L} ]+)*(?=\s|$)$'
  end
end
