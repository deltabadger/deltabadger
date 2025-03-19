module DomIdable
  extend ActiveSupport::Concern

  # mocks the dom_id method of ActionView::RecordIdentifier
  def dom_id(resource, prefix = nil)
    class_name = resource.class.name.underscore.gsub('/', '_')
    [prefix, class_name, resource.id].compact.join('_')
  end
end
