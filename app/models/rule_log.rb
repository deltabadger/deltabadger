class RuleLog < ApplicationRecord
  belongs_to :rule

  enum :status, %i[pending success failed]
end
