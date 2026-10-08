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
  # The tick's placement fence, read right before each order is sent. A stop, archive or delete can
  # commit while a claimed tick (:executing in this copy) sizes its orders, and a job already running
  # cannot be cancelled, so the tick asks the row, not this copy. A stop that commits after this read
  # still lets that one order go; the next leg is fenced. Only a claimed tick is fenced: nothing else
  # places through the tick's order setters.
  def stopped_mid_tick?
    executing? && !self.class.working.exists?(id)
  end

  def transition_working!(status)
    transaction do
      next false unless self.class.working.exists?(id)

      update!(status:)
      true
    end
  end
end
