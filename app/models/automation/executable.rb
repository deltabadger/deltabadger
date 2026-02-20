module Automation::Executable
  extend ActiveSupport::Concern

  def execute
    raise NotImplementedError, "#{self.class.name} must implement execute"
  end

  def api_key_type
    raise NotImplementedError, "#{self.class} must implement api_key_type"
  end

  def parse_params(params)
    raise NotImplementedError, "#{self.class.name} must implement parse_params"
  end

  def start(start_fresh: true)
    raise NotImplementedError, "#{self.class.name} must implement start"
  end

  def stop(stop_message_key: nil)
    raise NotImplementedError, "#{self.class.name} must implement stop"
  end

  def delete
    raise NotImplementedError, "#{self.class.name} must implement delete"
  end

  def destroy
    self.status = 'deleted'
    save(validate: false)
  end
end
