class ApiKey < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  attr_encrypted :key, key: ENV.fetch('API_KEY_ENCRRYPTION_KEY')
end
