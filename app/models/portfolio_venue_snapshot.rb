# One exchange's day, beside the whole portfolio's (`PortfolioSnapshot`): the same figures, read off
# the lots and balances that sit on that exchange. The venues of a day add up to the whole.
class PortfolioVenueSnapshot < ApplicationRecord
  belongs_to :user
  belongs_to :exchange

  scope :for_user, ->(user) { where(user_id: user.id) }
end
