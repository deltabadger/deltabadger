module Survey::Typeable
  extend ActiveSupport::Concern

  # We use this concern to get enum-like functionality for the type column
  # We can't use enum because we are using STI.

  included do
    scope :onboarding, -> { where(type: 'Surveys::Onboarding') }
  end

  def onboarding?
    type == 'Surveys::Onboarding'
  end
end
