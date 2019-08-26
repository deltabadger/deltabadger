class Bot < ApplicationRecord
  belongs_to :exchange
  belongs_to :user
end
