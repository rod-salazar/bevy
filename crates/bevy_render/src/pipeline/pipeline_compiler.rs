use super::{state_descriptors::PrimitiveTopology, IndexFormat, PipelineDescriptor};
use crate::{
    pipeline::{BindType, InputStepMode, VertexBufferDescriptor},
    renderer::RenderResourceContext,
    shader::{Shader, ShaderError, ShaderSource},
};
use bevy_asset::{Assets, Handle};
use bevy_reflect::Reflect;
use bevy_utils::{HashMap, HashSet};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

#[derive(Clone, Eq, PartialEq, Debug, Reflect)]
pub struct PipelineSpecialization {
    pub shader_specialization: ShaderSpecialization,
    pub primitive_topology: PrimitiveTopology,
    pub dynamic_bindings: HashSet<String>,
    pub index_format: IndexFormat,
    pub vertex_buffer_descriptors: Vec<VertexBufferDescriptor>,
    pub sample_count: u32,
}

impl Default for PipelineSpecialization {
    fn default() -> Self {
        Self {
            sample_count: 1,
            index_format: IndexFormat::Uint32,
            shader_specialization: Default::default(),
            primitive_topology: Default::default(),
            dynamic_bindings: Default::default(),
            vertex_buffer_descriptors: Default::default(),
        }
    }
}

impl PipelineSpecialization {
    pub fn empty() -> &'static PipelineSpecialization {
        pub static EMPTY: Lazy<PipelineSpecialization> = Lazy::new(PipelineSpecialization::default);
        &EMPTY
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Default, Reflect, Serialize, Deserialize)]
pub struct ShaderSpecialization {
    pub shader_defs: HashSet<String>,
}

#[derive(Debug)]
struct SpecializedShader {
    shader: Handle<Shader>,
    specialization: ShaderSpecialization,
}

#[derive(Debug)]
struct SpecializedPipeline {
    pipeline: Handle<PipelineDescriptor>,
    specialization: PipelineSpecialization,
}

#[derive(Debug, Default)]
pub struct PipelineCompiler {
    specialized_shaders: HashMap<Handle<Shader>, Vec<SpecializedShader>>,
    specialized_shader_pipelines: HashMap<Handle<Shader>, Vec<Handle<PipelineDescriptor>>>,
    specialized_pipelines: HashMap<Handle<PipelineDescriptor>, Vec<SpecializedPipeline>>,
}

impl PipelineCompiler {
    fn compile_shader(
        &mut self,
        render_resource_context: &dyn RenderResourceContext,
        shaders: &mut Assets<Shader>,
        shader_handle: &Handle<Shader>,
        // Given from the pipeline ... smart enough to know which shader defs are needed
        shader_specialization: &ShaderSpecialization,
    ) -> Result<Handle<Shader>, ShaderError> {
        // This is the only place where we actually insert into specialized_shaders.
        // This means that this call-site is where "specializations are "registered" or created.
        // We are given a shader asset handle and insert an empty vector as the value.
        let specialized_shaders = self
            .specialized_shaders
            .entry(shader_handle.clone_weak())
            .or_insert_with(Vec::new);

        // shader must exist, can't be None
        let shader = shaders.get(shader_handle).unwrap();

        // Rod: Here we exit early if we are already spirv. Unclear if we are trying
        // to get it into spirv or if spriv is just a special case that can't be worked with...
        // after reading... maybe you just can not create specialized versions of it.

        // don't produce new shader if the input source is already spirv
        if let ShaderSource::Spirv(_) = shader.source {
            return Ok(shader_handle.clone_weak());
        }

        if let Some(specialized_shader) =
            // We are going over all specialized_shaders, not just the one regarding the shader we are compiling here.
            specialized_shaders
            .iter()
            .find(|current_specialized_shader| {
                // can this be sped up? Hash of the HashSet?
                current_specialized_shader.specialization == *shader_specialization
            })
        {
            // if shader has already been compiled with current configuration, use existing shader
            Ok(specialized_shader.shader.clone_weak())
        } else {
            // if no shader exists with the current configuration, create new shader and compile
            let shader_def_vec = shader_specialization
                .shader_defs
                .iter()
                .cloned()
                .collect::<Vec<String>>();
            let compiled_shader =
                render_resource_context.get_specialized_shader(shader, Some(&shader_def_vec))?;
            let specialized_handle = shaders.add(compiled_shader);
            let weak_specialized_handle = specialized_handle.clone_weak();
            specialized_shaders.push(SpecializedShader {
                shader: specialized_handle,
                specialization: shader_specialization.clone(),
            });
            Ok(weak_specialized_handle)
        }
    }

    pub fn get_specialized_pipeline(
        &self,
        pipeline: &Handle<PipelineDescriptor>,
        specialization: &PipelineSpecialization,
    ) -> Option<Handle<PipelineDescriptor>> {
        self.specialized_pipelines
            .get(pipeline)
            .and_then(|specialized_pipelines| {
                specialized_pipelines
                    .iter()
                    .find(|current_specialized_pipeline| {
                        &current_specialized_pipeline.specialization == specialization
                    })
            })
            .map(|specialized_pipeline| specialized_pipeline.pipeline.clone_weak())
    }

    pub fn compile_pipeline(
        &mut self,
        render_resource_context: &dyn RenderResourceContext,
        pipelines: &mut Assets<PipelineDescriptor>,
        shaders: &mut Assets<Shader>,
        source_pipeline: &Handle<PipelineDescriptor>,
        pipeline_specialization: &PipelineSpecialization,
    ) -> Handle<PipelineDescriptor> {
        let source_descriptor = pipelines.get(source_pipeline).unwrap();
        let mut specialized_descriptor = source_descriptor.clone();
        let specialized_vertex_shader = self
            .compile_shader(
                render_resource_context,
                shaders,
                &specialized_descriptor.shader_stages.vertex,
                &pipeline_specialization.shader_specialization,
            )
            .unwrap();
        specialized_descriptor.shader_stages.vertex = specialized_vertex_shader.clone_weak();
        let mut specialized_fragment_shader = None;
        specialized_descriptor.shader_stages.fragment = specialized_descriptor
            .shader_stages
            .fragment
            .as_ref()
            .map(|fragment| {
                let shader = self
                    .compile_shader(
                        render_resource_context,
                        shaders,
                        fragment,
                        &pipeline_specialization.shader_specialization,
                    )
                    .unwrap();
                specialized_fragment_shader = Some(shader.clone_weak());
                shader
            });

        let mut layout = render_resource_context.reflect_pipeline_layout(
            &shaders,
            &specialized_descriptor.shader_stages,
            true,
        );

        if !pipeline_specialization.dynamic_bindings.is_empty() {
            // set binding uniforms to dynamic if render resource bindings use dynamic
            for bind_group in layout.bind_groups.iter_mut() {
                let mut binding_changed = false;
                for binding in bind_group.bindings.iter_mut() {
                    if pipeline_specialization
                        .dynamic_bindings
                        .iter()
                        .any(|b| b == &binding.name)
                    {
                        if let BindType::Uniform {
                            ref mut dynamic, ..
                        } = binding.bind_type
                        {
                            *dynamic = true;
                            binding_changed = true;
                        }
                    }
                }

                if binding_changed {
                    bind_group.update_id();
                }
            }
        }
        specialized_descriptor.layout = Some(layout);

        // create a vertex layout that provides all attributes from either the specialized vertex buffers or a zero buffer
        let mut pipeline_layout = specialized_descriptor.layout.as_mut().unwrap();
        // the vertex buffer descriptor of the mesh

        // Here we'd actually have multiple vertex buffers in the specialization and we wouldn't
        // call this a "mesh vertex buffer descriptor"., but rather a list of them.
        // then below in the loop we can flatten the pipeline_layout into 1 buffer descriptor
        // per specialization buffer descriptor.
        let mesh_vertex_buffer_descriptors = &pipeline_specialization.vertex_buffer_descriptors;
        let mut vertex_buffer_descriptors = Vec::<VertexBufferDescriptor>::default();

        println!("mesh_vertex_buffer_descriptor 1");
        for mesh_vertex_buffer_descriptor in mesh_vertex_buffer_descriptors {
            println!("mesh_vertex_buffer_descriptor 2");
            // the vertex buffer descriptor that will be used for this pipeline
            let mut compiled_vertex_buffer_descriptor = VertexBufferDescriptor {
                step_mode: InputStepMode::Vertex,
                stride: mesh_vertex_buffer_descriptor.stride,
                ..Default::default()
            };

            // This actually flattens the "reflected layout" which is in 1 vertex buffer descriptor per
            // shader vertex attribute and we flatten it down into 1 "compiled_vertex_buffer_descriptor"

            // If we ever want to put the undefined mesh attributes with a fallback buffer then here
            // we need to exclude the attributes that are not in mesh_vertex_buffer_descriptor from the
            // compiled_vertex_buffer_descriptor and put those attributes into a separate vertex buffer
            // descriptor.
            for shader_vertex_attribute in pipeline_layout.vertex_buffer_descriptors.iter() {
                let shader_vertex_attribute = shader_vertex_attribute
                    .attributes
                    .get(0)
                    .expect("Reflected layout has no attributes.");

                println!(
                    "for shader_vertex_attribute: {}",
                    shader_vertex_attribute.name
                );

                mesh_vertex_buffer_descriptor
                    .attributes
                    .iter()
                    .for_each(|x| println!("x: {}", x.name));

                if let Some(target_vertex_attribute) = mesh_vertex_buffer_descriptor
                    .attributes
                    .iter()
                    .find(|x| x.name == shader_vertex_attribute.name)
                {
                    // copy shader location from reflected layout
                    let mut compiled_vertex_attribute = target_vertex_attribute.clone();
                    println!(
                        "Shader location: {} ",
                        shader_vertex_attribute.shader_location
                    );
                    compiled_vertex_attribute.shader_location =
                        shader_vertex_attribute.shader_location;
                    compiled_vertex_buffer_descriptor
                        .attributes
                        .push(compiled_vertex_attribute);
                } else {
                    // panic!(
                    //     "Attribute {} is required by shader, but not supplied by mesh. Either remove the attribute from the shader or supply the attribute ({}) to the mesh.",
                    //     shader_vertex_attribute.name,
                    //     shader_vertex_attribute.name,
                    // );
                }
            }

            //TODO: add other buffers (like instancing) here

            // These "compiled_vertex_buffer_descriptor" attributes came from parsing the shaders themselves.
            // We add this as 1 single vertex buffer descriptor ohnto the pipeline_layout.vertex_buffer_descriptors.
            // Looks like it gets FLATTENED here into 1.
            vertex_buffer_descriptors.push(compiled_vertex_buffer_descriptor);
        }

        println!(
            "pipeline layout v buf desc size: {}",
            vertex_buffer_descriptors.len()
        );
        pipeline_layout.vertex_buffer_descriptors = vertex_buffer_descriptors;
        specialized_descriptor.sample_count = pipeline_specialization.sample_count;
        specialized_descriptor.primitive_topology = pipeline_specialization.primitive_topology;
        specialized_descriptor.index_format = pipeline_specialization.index_format;

        let specialized_pipeline_handle = pipelines.add(specialized_descriptor);
        render_resource_context.create_render_pipeline(
            specialized_pipeline_handle.clone_weak(),
            pipelines.get(&specialized_pipeline_handle).unwrap(),
            &shaders,
        );

        // track specialized shader pipelines
        self.specialized_shader_pipelines
            .entry(specialized_vertex_shader)
            .or_insert_with(Default::default)
            .push(source_pipeline.clone_weak());
        if let Some(specialized_fragment_shader) = specialized_fragment_shader {
            self.specialized_shader_pipelines
                .entry(specialized_fragment_shader)
                .or_insert_with(Default::default)
                .push(source_pipeline.clone_weak());
        }

        let specialized_pipelines = self
            .specialized_pipelines
            .entry(source_pipeline.clone_weak())
            .or_insert_with(Vec::new);
        let weak_specialized_pipeline_handle = specialized_pipeline_handle.clone_weak();
        specialized_pipelines.push(SpecializedPipeline {
            pipeline: specialized_pipeline_handle,
            specialization: pipeline_specialization.clone(),
        });

        weak_specialized_pipeline_handle
    }

    pub fn iter_compiled_pipelines(
        &self,
        pipeline_handle: Handle<PipelineDescriptor>,
    ) -> Option<impl Iterator<Item = &Handle<PipelineDescriptor>>> {
        if let Some(compiled_pipelines) = self.specialized_pipelines.get(&pipeline_handle) {
            Some(
                compiled_pipelines
                    .iter()
                    .map(|specialized_pipeline| &specialized_pipeline.pipeline),
            )
        } else {
            None
        }
    }

    pub fn iter_all_compiled_pipelines(&self) -> impl Iterator<Item = &Handle<PipelineDescriptor>> {
        self.specialized_pipelines
            .values()
            .map(|compiled_pipelines| {
                compiled_pipelines
                    .iter()
                    .map(|specialized_pipeline| &specialized_pipeline.pipeline)
            })
            .flatten()
    }

    /// Update specialized shaders and remove any related specialized
    /// pipelines and assets.
    pub fn update_shader(
        &mut self,
        shader: &Handle<Shader>,
        pipelines: &mut Assets<PipelineDescriptor>,
        shaders: &mut Assets<Shader>,
        render_resource_context: &dyn RenderResourceContext,
    ) -> Result<(), ShaderError> {
        if let Some(specialized_shaders) = self.specialized_shaders.get_mut(shader) {
            for specialized_shader in specialized_shaders {
                // Recompile specialized shader. If it fails, we bail immediately.
                let shader_def_vec = specialized_shader
                    .specialization
                    .shader_defs
                    .iter()
                    .cloned()
                    .collect::<Vec<String>>();
                let new_handle =
                    shaders.add(render_resource_context.get_specialized_shader(
                        shaders.get(shader).unwrap(),
                        Some(&shader_def_vec),
                    )?);

                // Replace handle and remove old from assets.
                let old_handle = std::mem::replace(&mut specialized_shader.shader, new_handle);
                shaders.remove(&old_handle);

                // Find source pipelines that use the old specialized
                // shader, and remove from tracking.
                if let Some(source_pipelines) =
                    self.specialized_shader_pipelines.remove(&old_handle)
                {
                    // Remove all specialized pipelines from tracking
                    // and asset storage. They will be rebuilt on next
                    // draw.
                    for source_pipeline in source_pipelines {
                        if let Some(specialized_pipelines) =
                            self.specialized_pipelines.remove(&source_pipeline)
                        {
                            for p in specialized_pipelines {
                                pipelines.remove(p.pipeline);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
