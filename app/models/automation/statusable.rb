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

  # A tick's own status write, made only while the bot is still working. A stop is a separate request
  # that can land while the tick waits on the exchange; a plain update! would undo it and the bot
  # would keep trading. The check and the write share one IMMEDIATE transaction, so a stop committed
  # before it always wins. Returns whether the bot moved.
  def transition_working!(status)
    transaction do
      next false unless self.class.working.exists?(id)

      update!(status:)
      true
    end
  end
end
