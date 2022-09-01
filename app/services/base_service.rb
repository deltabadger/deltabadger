require 'result'

class BaseService
  def self.call(*args)
    puts "==========================="
    puts "=========== #{args.inspect}================"
    puts "==========================="
    # return if args.empty?
    new.call(*args)
  end

  def call(args)
    raise NotImplementedError
  end
end
