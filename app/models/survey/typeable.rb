module Survey::Typeable
  extend ActiveSupport::Concern

  # We use this concern to get enum-like functionality for the type column
  # We can't use enum because we are using STI.

  included do
    scope :onboarding, -> { where(type: 'Surveys::Onboarding') }
    scope :onboarding_v2, -> { where(type: 'Surveys::OnboardingV2') }
  end

  def onboarding?
    type == 'Surveys::Onboarding'
  end

  def onboarding_v2?
    type == 'Surveys::OnboardingV2'
  end
end
