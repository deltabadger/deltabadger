class Result
  attr_reader :data, :errors
  def initialize(data:, errors:)
    @data = data
    @errors = errors
  end

  def success?
    errors.empty?
  end

  def failure?
    !success?
  end

  class Success < Result
    def initialize(data = nil)
      @data = data
      @errors = []
    end
  end

  class Failure < Result
    def initialize(*errors)
      @data = nil
      @errors = errors.empty? ? ['Error'] : errors
    end
  end
end
