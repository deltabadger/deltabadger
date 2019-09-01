require 'result'

class BaseService
  def self.call(*args)
    new.call(*args)
  end

  def call
    raise NotImplementedError
  end
end
