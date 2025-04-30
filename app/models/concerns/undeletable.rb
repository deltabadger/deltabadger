module Undeletable
  extend ActiveSupport::Concern

  included do
    before_destroy :prevent_destruction

    class_eval do
      class << self
        alias_method :original_delete, :delete
        alias_method :original_delete_all, :delete_all

        def delete(_id)
          raise ActiveRecord::RecordNotDestroyed, "Direct deletion via 'delete' is not allowed on #{name}"
        end

        def delete_all(*_args)
          raise ActiveRecord::RecordNotDestroyed, "Bulk deletion via 'delete_all' is not allowed on #{name}"
        end
      end
    end
  end

  def delete
    raise ActiveRecord::RecordNotDestroyed, "Direct deletion via 'delete' is not allowed on #{self.class.name}"
  end

  private

  def prevent_destruction
    errors.add(:base, 'This record cannot be deleted')
    throw :abort
  end
end
