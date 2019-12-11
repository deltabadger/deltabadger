class ActiveRecordDecorator < SimpleDelegator
  # A proxy for the decorator class to allow the delegation of certain class
  # methods to the decorated object's class.
  class ClassProxy < SimpleDelegator
    def initialize(decorator_class, decorated_class)
      super decorator_class
      self.decorated_class = decorated_class
    end

    # Redefine to allow assignment of decorated Active Record objects to
    # associations, which expect `#primary_key` to be defined on the class.
    def primary_key
      decorated_class.primary_key
    end

    protected

    attr_accessor :decorated_class
  end

  # Wraps the decorator's class in a proxy to allow decoration of the decorated
  # object's class.
  def class
    ClassProxy.new(super, __getobj__.class)
  end

  # Redefines to avoid ActiveRecord::AssociationTypeMismatch errors when
  # assigning decorated Active Record models to associations.
  def is_a?(klass)
    __getobj__.is_a?(klass)
  end
end
