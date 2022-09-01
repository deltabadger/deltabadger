class BaseService
  def self.call(...)
    new.call(...)
  end

  def call(args)
    raise NotImplementedError
  end
end