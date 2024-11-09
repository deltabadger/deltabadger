class User::Password
  LOWERCASE_PATTERN = '(?=.*[a-z])'.freeze         # At least one lowercase letter
  UPPERCASE_PATTERN = '(?=.*[A-Z])'.freeze         # At least one uppercase letter
  DIGIT_PATTERN = '(?=.*\d)'.freeze                # At least one digit
  SYMBOL_PATTERN = '(?=.*[\W_])'.freeze            # At least one symbol (non-word character or underscore)
  LENGTH_PATTERN = ".{#{Devise.password_length.min},}".freeze  # At least minimum_length characters
  PATTERN = "#{LOWERCASE_PATTERN}#{UPPERCASE_PATTERN}#{DIGIT_PATTERN}#{SYMBOL_PATTERN}#{LENGTH_PATTERN}".freeze
end
