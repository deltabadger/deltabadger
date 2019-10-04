class Subscriber < ApplicationRecord
  validates :email,
            presence: true,
            uniqueness: { message: 'already used' },
            format: { with: Devise.email_regexp }
end
