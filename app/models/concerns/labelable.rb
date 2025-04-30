module Labelable
  extend ActiveSupport::Concern

  included do
    validates :label, presence: true

    after_initialize :set_default_label, if: -> { label.blank? }
    after_find :ensure_label_exists
  end

  private

  def set_default_label
    self.label = generate_label
  end

  def ensure_label_exists
    return unless label.blank?

    update!(label: generate_label)
  end

  def generate_label
    label = nil
    max_attempts = 100
    loop do
      label = Haikunator.haikunate(0, ' ').titleize
      break unless self.class.unscoped.exists?(label: label, user_id: user_id)

      max_attempts -= 1
      break if max_attempts.zero?
    end

    label
  end
end
