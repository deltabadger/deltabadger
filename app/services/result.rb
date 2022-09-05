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

  def or(other)
    return data if success?

    other
  end

  def ==(other)
    return false unless other.is_a?(Result)

    data == other.data && errors == other.errors
  end

  class Success < Result
    def initialize(data = nil)
      @data = data
      @errors = []
    end
  end

  class Failure < Result
    def initialize(*errors, **kwargs)
      @data = kwargs[:data]
      @errors = errors.empty? ? ['Error'] : errors
    end
  end
end
