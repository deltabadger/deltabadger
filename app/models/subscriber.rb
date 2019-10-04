class Subscriber < ApplicationRecord
  validates :email,
            presence: true,
            uniqueness: true,
            format: { with: Devise.email_regexp }
end
