module Automation::Statusable
  extend ActiveSupport::Concern

  included do
    enum :status, %i[created scheduled stopped deleted executing retrying waiting]

    scope :working, -> { where(status: %i[scheduled executing retrying waiting]) }
  end

  def working?
    scheduled? || executing? || retrying? || waiting?
  end
end
