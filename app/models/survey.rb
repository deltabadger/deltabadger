class Survey < ApplicationRecord
  belongs_to :user

  validates :answers, presence: true

  include Typeable
end
