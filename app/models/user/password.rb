class User::Password
  def self.minimum_length
    Devise.password_length.min
  end

  def self.lowercase_pattern
    '(?=.*[a-z])'            # At least one lowercase letter
  end

  def self.uppercase_pattern
    '(?=.*[A-Z])'            # At least one uppercase letter
  end

  def self.digit_pattern
    '(?=.*\d)'               # At least one digit
  end

  def self.symbol_pattern
    '(?=.*[\W_])'            # At least one symbol (non-word character or underscore)
  end

  def self.length_pattern
    ".{#{minimum_length},}"  # At least minimum_length characters
  end

  def self.complexity_pattern
    "^#{lowercase_pattern}#{uppercase_pattern}#{digit_pattern}#{symbol_pattern}#{length_pattern}$"
  end
end
