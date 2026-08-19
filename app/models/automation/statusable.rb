module Automation::Statusable
  extend ActiveSupport::Concern

  included do
    # `archived` is appended, never inserted: the column stores the position.
    enum :status, %i[created scheduled stopped deleted executing retrying waiting archived]

    scope :working, -> { where(status: %i[scheduled executing retrying waiting]) }
  end

  def working?
    scheduled? || executing? || retrying? || waiting?
  end
end
