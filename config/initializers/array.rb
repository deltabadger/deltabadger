# FIXME: this is a dirty hack to make the Array compact_blank! method available
# within ruby 3.0, this file must be removed after upgrading to ruby 3.1

class Array
  def compact_blank!
    replace(reject(&:blank?))
  end
end
