class Bots::Webhook < Bot
  include LegacyMethods

  def restarting?
    false
  end

  def restarting_within_interval?
    false
  end
end
